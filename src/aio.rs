//! Asynchonous I/O operaions.
use std::{
	hash::{Hash, Hasher},
	os::raw::c_void,
	panic::catch_unwind,
	ptr::{self, NonNull},
	sync::{
		atomic::{AtomicPtr, AtomicUsize, Ordering},
		Arc,
	},
	time::Duration,
};

use crate::{
	ctx::Context,
	error::{Error, Result, SendResult},
	message::Message,
	socket::Socket,
	util::{duration_to_nng, validate_ptr},
};
use log::error;

/// An asynchronous I/O context.
///
/// Asynchronous operations are performed without blocking calling application
/// threads. Instead the application registers a “callback” function to be
/// executed when the operation is complete (whether successfully or not). This
/// callback will be executed exactly once.
///
/// The callback must not perform any blocking operations and must complete it’s
/// execution quickly. If the callback does block, this can lead ultimately to
/// an apparent "hang" or deadlock in the application.
///
/// ## Example
///
/// A simple server that will sleep for the requested number of milliseconds
/// before responding:
///
/// ```
/// use std::time::Duration;
/// use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
/// use nng::*;
///
/// const ADDRESS: &'static str = "inproc://nng/aio/example";
/// const WORKERS: usize = 10;
///
/// fn server() -> Result<()> {
///     // Set up the server socket but don't listen for connections yet.
///     let server = Socket::new(Protocol::Rep0)?;
///
///     // Create all of the worker contexts. These do *not* represent the number
///     // of threads that the REP socket will use.
///     let workers: Vec<_> = (0..WORKERS)
///         .map(|_| {
///             let ctx = Context::new(&server)?;
///             let ctx_clone = ctx.clone();
///
///             // An actual program should have better error handling.
///             let aio = Aio::new(move |aio, res| callback(&aio, &ctx_clone, res).unwrap())?;
///             Ok((aio, ctx))
///         })
///         .collect::<Result<_>>()?;
///
///     // Only after we have all of the workers do we start listening.
///     server.listen(ADDRESS)?;
///
///     // Now, start the workers.
///     for (a, c) in &workers {
///         c.recv(a)?;
///     }
///
///     // Now, do nothing and let the workers handle the jobs.
///     std::thread::park();
///     Ok(())
/// }
///
/// fn callback(aio: &Aio, ctx: &Context, res: AioResult) -> Result<()> {
///     match res {
///         // We successfully send the reply, wait for a new request.
///         AioResult::SendOk => ctx.recv(aio),
///
///         // We successfully received a message.
///         AioResult::RecvOk(m) => {
///             let ms = m.as_slice().read_u64::<LittleEndian>().unwrap();
///             aio.sleep(Duration::from_millis(ms))
///         },
///
///         // We successfully slept.
///         AioResult::SleepOk => {
///             // We could have hung on to the request `Message` to avoid an
///             let _ = ctx.send(aio, Message::new()?)?;
///             Ok(())
///         },
///
///         // Anything else is an error and an actual program should handle it.
///         _ => panic!("Error in the AIO"),
///     }
/// }
///
/// fn client(ms: u64) -> Result<()> {
///     // Set up the client socket and connect to the server.
///     let client = Socket::new(Protocol::Req0)?;
///     client.dial(ADDRESS)?;
///     // Create the message containing the number of milliseconds to sleep.
///     let mut req = Message::new()?;
///     req.write_u64::<LittleEndian>(ms).unwrap();
///
///     // Send the request to the server and wait for a response.
///     client.send(req)?;
///
///     // This should block for approximately `ms` milliseconds as we wait for the
///     // server to sleep.
///     client.recv()?;
///
///     Ok(())
/// }
///
/// # // The async of this makes it hard to test, so we won't
/// ```
#[derive(Clone, Debug)]
pub struct Aio
{
	/// The inner AIO bits shared by all instances of this AIO.
	inner: Arc<Inner>,
}

impl Aio
{
	/// Creates a new asynchronous I/O handle.
	///
	/// The provided callback will be called on every single I/O event,
	/// successful or not. It is possible that the callback will be entered
	/// multiple times simultaneously.
	///
	/// ## Panicking
	///
	/// If the callback function panics, the program will abort. This is to
	/// match the behavior specified in Rust 1.33 where the program will abort
	/// when it panics across an `extern "C"` boundary. This library will
	/// produce the abort regardless of which version of Rustc is being used.
	///
	/// The user is responsible for either having a callback that never panics
	/// or catching and handling the panic within the callback.
	pub fn new<F>(callback: F) -> Result<Self>
	where
		F: Fn(Aio, AioResult) + Sync + Send + 'static,
	{
		// The shared inner needs to have a fixed location before we can do anything
		// else, which complicates the process of building the AIO slightly. We need to
		// use a second, non-atomic pointer and then atomically copy it in.
		let inner = Arc::new(Inner {
			handle: AtomicPtr::new(ptr::null_mut()),
			state:  AtomicUsize::new(State::Inactive as usize),
			callback: AtomicPtr::new(ptr::null_mut()),
		});

		// Now, we create the weak reference to the inner bits that will be stored
		// inside of the callback.
		let weak = Arc::downgrade(&inner);

		// Wrap the user's callback in our own state-keeping logic
		let bounce = move || {
			// If we can't upgrade the pointer, then we are in the middle of dropping,
			// so we can't do anything except return.
			let cb_aio = match weak.upgrade() {
				Some(i) => Aio { inner: i },
				None => return,
			};

			let res = unsafe {
				let state = cb_aio.inner.state.load(Ordering::Acquire).into();
				let aiop = cb_aio.inner.handle.load(Ordering::Relaxed);
				let rv = nng_sys::nng_aio_result(aiop) as u32;

				let res = match (state, rv) {
					(State::Sending, 0) => AioResult::SendOk,
					(State::Sending, e) => {
						let msgp = nng_sys::nng_aio_get_msg(aiop);
						let msg = Message::from_ptr(NonNull::new(msgp).unwrap());
						AioResult::SendErr(msg, Error::from_code(e))
					},

					(State::Receiving, 0) => {
						let msgp = nng_sys::nng_aio_get_msg(aiop);
						let msg = Message::from_ptr(NonNull::new(msgp).unwrap());
						AioResult::RecvOk(msg)
					},
					(State::Receiving, e) => AioResult::RecvErr(Error::from_code(e)),

					(State::Sleeping, 0) => AioResult::SleepOk,
					(State::Sleeping, e) => AioResult::SleepErr(Error::from_code(e)),

					// I am 99% sure that we will never get a callback in the Inactive state
					(State::Inactive, _) => unreachable!(),
				};

				cb_aio.inner.state.store(State::Inactive as usize, Ordering::Release);
				res
			};
			callback(cb_aio, res)
		};

		// There are ways to avoid the double boxing, but unfortunately storing
		// the callback inside of the Inner object means that we will need some
		// way to mutate it and all of those options require `Sized`, which in
		// turn means it needs a box.
		let boxed: Box<Box<dyn Fn() + Sync + Send + 'static>> = Box::new(Box::new(bounce));
		let callback_ptr = Box::into_raw(boxed);

		let mut aio: *mut nng_sys::nng_aio = ptr::null_mut();
		let aiop: *mut *mut nng_sys::nng_aio = &mut aio as _;
		let rv = unsafe{
			nng_sys::nng_aio_alloc(aiop, Some(Aio::trampoline), callback_ptr as _)
		};

		// NNG should never touch the pointer and return a non-zero code at the same
		// time. That being said, I'm going to be a pessimist and double check. If we do
		// encounter that case, the safest thing to do is make the pointer null again so
		// that the dropping of the inner can detect that something went south.
		//
		// This might leak memory (I'm not sure, depends on what NNG did), but a small
		// amount of lost memory is better than a segfaulting Rust library.
		if rv != 0 && !aio.is_null() {
			error!("NNG returned a non-null pointer from a failed function");
			return Err(Error::Unknown(0));
		}
		validate_ptr(rv, aio)?;
		inner.handle.store(aio, Ordering::Release);
		inner.callback.store(callback_ptr, Ordering::Relaxed);

		Ok(Self { inner })
	}

	/// Set the timeout of asynchronous operations.
	///
	/// This causes a timer to be started when the operation is actually
	/// started. If the timer expires before the operation is completed, then it
	/// is aborted with `Error::TimedOut`.
	///
	/// As most operations involve some context switching, it is usually a good
	/// idea to allow a least a few tens of milliseconds before timing them out
	/// - a too small timeout might not allow the operation to properly begin
	/// before giving up!
	///
	/// It is only valid to try and set this when no operations are active.
	pub fn set_timeout(&self, dur: Option<Duration>) -> Result<()>
	{
		// We need to check that no operations are happening and then prevent them from
		// happening while we set the timeout. Any state that isn't `Inactive` will do
		// so the choice is arbitrary. That being said, `Sleeping` feels the most
		// accurate.
		let sleeping = State::Sleeping as usize;
		let inactive = State::Inactive as usize;
		let old_state = self.inner.state.compare_and_swap(inactive, sleeping, Ordering::Acquire);

		if old_state == inactive {
			let ms = duration_to_nng(dur);
			let aiop = self.inner.handle.load(Ordering::Relaxed);
			unsafe {
				nng_sys::nng_aio_set_timeout(aiop, ms);
			}

			self.inner.state.store(inactive, Ordering::Release);
			Ok(())
		}
		else {
			// Should this be `Error::TryAgain`?
			Err(Error::IncorrectState)
		}
	}

	/// Performs and asynchronous sleep operation.
	///
	/// If the sleep finishes completely, it will never return an error. If a
	/// timeout has been set and it is shorter than the duration of the sleep
	/// operation, the sleep operation will end early with
	/// `Error::TimedOut`.
	///
	/// This function will return immediately. If there is already an I/O
	/// operation in progress, this function will return `Error::TryAgain`.
	pub fn sleep(&self, dur: Duration) -> Result<()>
	{
		let sleeping = State::Sleeping as usize;
		let inactive = State::Inactive as usize;
		let old_state = self.inner.state.compare_and_swap(inactive, sleeping, Ordering::AcqRel);

		if old_state == inactive {
			let ms = duration_to_nng(Some(dur));
			let aiop = self.inner.handle.load(Ordering::Relaxed);
			unsafe {
				nng_sys::nng_sleep_aio(ms, aiop);
			}

			Ok(())
		}
		else {
			Err(Error::TryAgain)
		}
	}

	/// Blocks the current thread until the current asynchronous operation
	/// completes.
	///
	/// If there are no operations running then this function returns
	/// immediately. This function should **not** be called from within the
	/// completion callback.
	pub fn wait(&self)
	{
		unsafe {
			nng_sys::nng_aio_wait(self.inner.handle.load(Ordering::Relaxed));
		}
	}

	/// Cancel the currently running I/O operation.
	pub fn cancel(&self)
	{
		unsafe {
			nng_sys::nng_aio_cancel(self.inner.handle.load(Ordering::Relaxed));
		}
	}

	/// Send a message on the provided socket.
	pub(crate) fn send_socket(&self, socket: &Socket, msg: Message) -> SendResult<()>
	{
		let inactive = State::Inactive as usize;
		let sending = State::Sending as usize;

		let old_state = self.inner.state.compare_and_swap(inactive, sending, Ordering::AcqRel);

		if old_state == inactive {
			let aiop = self.inner.handle.load(Ordering::Relaxed);
			unsafe {
				nng_sys::nng_aio_set_msg(aiop, msg.into_ptr().as_ptr());
				nng_sys::nng_send_aio(socket.handle(), aiop);
			}

			Ok(())
		}
		else {
			Err((msg, Error::TryAgain))
		}
	}

	/// Receive a message on the provided socket.
	pub(crate) fn recv_socket(&self, socket: &Socket) -> Result<()>
	{
		let inactive = State::Inactive as usize;
		let receiving = State::Receiving as usize;
		let old_state = self.inner.state.compare_and_swap(inactive, receiving, Ordering::AcqRel);

		if old_state == inactive {
			let aiop = self.inner.handle.load(Ordering::Relaxed);
			unsafe {
				nng_sys::nng_recv_aio(socket.handle(), aiop);
			}
			Ok(())
		}
		else {
			Err(Error::TryAgain)
		}
	}

	/// Send a message on the provided context.
	pub(crate) fn send_ctx(&self, ctx: &Context, msg: Message) -> SendResult<()>
	{
		let inactive = State::Inactive as usize;
		let sending = State::Sending as usize;

		let old_state = self.inner.state.compare_and_swap(inactive, sending, Ordering::AcqRel);

		if old_state == inactive {
			let aiop = self.inner.handle.load(Ordering::Relaxed);
			unsafe {
				nng_sys::nng_aio_set_msg(aiop, msg.into_ptr().as_ptr());
				nng_sys::nng_ctx_send(ctx.handle(), aiop);
			}

			Ok(())
		}
		else {
			Err((msg, Error::TryAgain))
		}
	}

	/// Receive a message on the provided context.
	pub(crate) fn recv_ctx(&self, ctx: &Context) -> Result<()>
	{
		let inactive = State::Inactive as usize;
		let receiving = State::Receiving as usize;
		let old_state = self.inner.state.compare_and_swap(inactive, receiving, Ordering::AcqRel);

		if old_state == inactive {
			let aiop = self.inner.handle.load(Ordering::Relaxed);
			unsafe {
				nng_sys::nng_ctx_recv(ctx.handle(), aiop);
			}
			Ok(())
		}
		else {
			Err(Error::TryAgain)
		}
	}

	/// Trampoline function for calling a closure from C.
	///
	/// This is really unsafe because you have to be absolutely positive in that
	/// the type of the pointer is actually `F`. Because we're going through C
	/// and a `c_void`, the type system does not enforce this for us.
	extern "C" fn trampoline(arg: *mut c_void)
	{
		let res = catch_unwind(|| unsafe {
			let callback_ptr = arg as *const Box<dyn Fn() + Sync + Send + 'static>;
			if callback_ptr.is_null() {
				// This should never happen. It means we, Nng-rs, got something wrong in the
				// allocation code.
				panic!("Null argument given to trampoline function - please open an issue");
			}

			(*callback_ptr)()
		});

		// See #6 for "discussion" about why we abort here.
		if res.is_err() {
			// No other useful information to relay to the user.
			error!("Panic in AIO callback function.");
			std::process::abort();
		}
	}
}

impl Hash for Aio
{
	fn hash<H: Hasher>(&self, state: &mut H)
	{
		self.inner.handle.load(Ordering::Relaxed).hash(state)
	}
}

impl PartialEq for Aio
{
	fn eq(&self, other: &Aio) -> bool
	{
		self.inner.handle.load(Ordering::Relaxed) == other.inner.handle.load(Ordering::Relaxed)
	}
}

impl Eq for Aio {}

/// The shared inner items of a `Aio`.
#[derive(Debug)]
struct Inner
{
	/// The handle to the NNG AIO object.
	///
	/// Unfortunately, we do have to put this behind some kind of
	/// synchronization primitive. Fortunately, we can always access it with
	/// with the Relaxed ordering and, because we're almost always accessing the
	/// state atomic when we access the handle, we shouldn't have any
	/// extra cache issues.
	handle: AtomicPtr<nng_sys::nng_aio>,

	/// The current state of the AIO object, represented as a `usize`.
	state: AtomicUsize,

	/// The callback function.
	///
	/// We're OK with the extra layer of indirection because we never call it.
	callback: AtomicPtr<Box<dyn Fn() + Sync + Send + 'static>>,
}

impl Drop for Inner
{
	fn drop(&mut self)
	{
		// It is possible for this to be dropping while the pointer is null. The
		// Inner struct is created before the pointer is allocated and it will be
		// dropped with a null pointer if the NNG allocation fails.
		let aiop = self.handle.load(Ordering::Acquire);
		if !aiop.is_null() {
			// If the callback has started, it will not be able to upgrade the weak pointer
			// to a strong one and so it will just return from the callback. Otherwise, the
			// NNG call to stop the AIO will wait until all callbacks have completed and it
			// will prevent any more operations from starting.
			//
			// I think the call to free will do the same thing as the stop, but the online
			// docs aren't super clear, the header has a comment saying that the AIO must
			// not be running an operation when free is called, and the source doesn't
			// clearly (to my understanding of the code) show that it is being done. Plus,
			// the manual does suggest cases where stopping first is good.
			unsafe {
				nng_sys::nng_aio_stop(aiop);
				nng_sys::nng_aio_free(aiop);

				// Now that we know nothing is in the callback, we can free it.
				let _ = Box::from_raw(self.callback.load(Ordering::Relaxed));
			}
		}
	}
}

/// The result of an AIO operation.
// There are no "Inactive" results as I don't think there is a valid way to get any type of callback
// trigger when there are no operations running. All of the "user forced" errors, such as
// cancellation or timeouts, don't happen if there are no running operations. If there are no
// running operations, then no non-"user forced" errors can happen.
#[derive(Clone, Debug)]
#[must_use]
pub enum AioResult
{
	/// The send operation was successful.
	SendOk,

	/// The send operation failed.
	///
	/// This contains the message that was being sent.
	SendErr(Message, Error),

	/// The receive operation was successful.
	RecvOk(Message),

	/// The receive operation failed.
	RecvErr(Error),

	/// The sleep operation was successful.
	SleepOk,

	/// The sleep operation failed.
	///
	/// This is almost always because the sleep was canceled and the error will
	/// usually be `Error::Canceled`.
	SleepErr(Error),
}

impl From<AioResult> for Result<Option<Message>>
{
	fn from(aio_res: AioResult) -> Result<Option<Message>>
	{
		use self::AioResult::*;

		match aio_res {
			SendOk | SleepOk => Ok(None),
			SendErr(_, e) | RecvErr(e) | SleepErr(e) => Err(e),
			RecvOk(m) => Ok(Some(m)),
		}
	}
}

/// Represents the state of the AIO object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
enum State
{
	/// There is currently nothing happening on the AIO.
	Inactive,

	/// A send operation is currently in progress.
	Sending,

	/// A receive operation is currently in progress.
	Receiving,

	/// The AIO object is currently sleeping.
	Sleeping,
}

impl From<usize> for State
{
	fn from(atm: usize) -> State
	{
		// Fortunately, Godbolt says that this will compile to a compare, jump, and a
		// subtract. Three instructions isn't that bad.
		match atm {
			x if x == State::Inactive as usize => State::Inactive,
			x if x == State::Sending as usize => State::Sending,
			x if x == State::Receiving as usize => State::Receiving,
			x if x == State::Sleeping as usize => State::Sleeping,
			_ => unreachable!(),
		}
	}
}
