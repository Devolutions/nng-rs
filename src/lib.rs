//! A safe Rust wrapper for NNG
//!
//! ## What Is NNG
//!
//! From the [NNG Github Repository][1]:
//!
//! > NNG, like its predecessors nanomsg (and to some extent ZeroMQ), is a
//! lightweight, broker-less library, offering a simple API to solve common
//! recurring messaging problems, such as publish/subscribe, RPC-style
//! request/reply, or service discovery. The API frees the programmer
//! from worrying about details like connection management, retries, and other
//! common considerations, so that they can focus on the application instead of
//! the plumbing.
//!
//! ## Nng-rs
//!
//! This crate provides a safe wrapper around the NNG library, seeking to
//! maintain an API that is similar to the original library. As such, the
//! majority of examples available online should be easy to apply to this crate.
//!
//! ### Examples
//!
//! The following example uses the [intra-process][2] transport to set up a
//! [request][3]/[reply][4] socket pair. The "client" sends a String to the
//! "server" which responds with a nice phrase.
//!
//! ```
//! use std::io::Write;
//! use nng::*;
//!
//! const ADDRESS: &'static str = "inproc://nng/example";
//!
//! fn request() -> Result<()> {
//!     // Set up the client and connect to the specified address
//!     let mut client = Socket::new(Protocol::Req0)?;
//!     # // Don't error if we hit here before the server does.
//!     # client.set_nonblocking(true);
//!     client.dial(ADDRESS)?;
//!
//!     // Send the request from the client to the server. In general, it will be
//!     // better to directly use a `Message` to enable zero-copy, but that doesn't
//!     // matter here.
//!     # // Do block until the server is ready.
//!     # client.set_nonblocking(false);
//!     client.send("Ferris".as_bytes())?;
//!
//!     // Wait for the response from the server.
//!     let msg = client.recv()?;
//!     let reply = String::from_utf8_lossy(&msg);
//!     assert_eq!(reply, "Hello, Ferris!");
//!     Ok(())
//! }
//!
//! fn reply() -> Result<()> {
//!     // Set up the server and listen for connections on the specified address.
//!     let server = Socket::new(Protocol::Rep0)?;
//!     server.listen(ADDRESS)?;
//!
//!     // Receive the message from the client.
//!     let mut msg = server.recv()?;
//!     let name = String::from_utf8_lossy(&msg).into_owned();
//!     assert_eq!(name, "Ferris");
//!
//!     // Reuse the message to be more efficient.
//!     msg.clear();
//!     write!(msg, "Hello, {}!", name).unwrap();
//!
//!     server.send(msg)?;
//!     Ok(())
//! }
//!
//! # // Start the server first, so the client can connect to it.
//! # let jh = std::thread::spawn(|| reply().unwrap());
//! # request().unwrap();
//! # jh.join().unwrap();
//! ```
//!
//! Additional examples are in the `examples` directory.
//!
//! [1]: https://github.com/nanomsg/nng
//! [2]: https://nanomsg.github.io/nng/man/v1.1.0/nng_inproc.7
//! [3]: https://nanomsg.github.io/nng/man/v1.1.0/nng_req.7
//! [4]: https://nanomsg.github.io/nng/man/v1.1.0/nng_rep.7

// The following lints are of critical importance.
#![forbid(improper_ctypes)]
// Utilize Clippy to try and keep this crate clean. At some point (cargo#5034, I think?) this
// specification should be possible in either the Clippy TOML file or in the Cargo TOML file. These
// should be moved there once possible.
#![deny(bare_trait_objects)]
#![deny(missing_debug_implementations)]
#![deny(missing_docs)]
#![deny(unreachable_pub)]
#![deny(clippy::all)]
#![deny(clippy::wrong_pub_self_convention)]
// Clippy doesn't enable these with "all". Best to keep them warnings.
#![warn(clippy::nursery)]
#![warn(clippy::pedantic)]
#![warn(clippy::cargo)]
#![warn(clippy::clone_on_ref_ptr)]
#![warn(clippy::decimal_literal_representation)]
#![warn(clippy::print_stdout)]
#![warn(clippy::unimplemented)]
#![warn(clippy::use_debug)]
// I would like to be able to keep these on, but due to the nature of the crate it just isn't
// feasible. For example, the "cast_sign_loss" will warn at every i32/u32 conversion. Normally, I
// would like that, but this library is a safe wrapper around a Bindgen-based binding of a C
// library, which means the types are a little bit up-in-the-air.
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::empty_enum)] // Revisit after RFC1861 and RFC1216.
#![allow(clippy::cargo_common_metadata)] // Can't control this.
#![allow(clippy::module_name_repetitions)] // Doesn't recognize public re-exports.

// In these cases, I just don't like what Clippy suggests.
#![allow(clippy::use_self)]
#![allow(clippy::replace_consts)]
#![allow(clippy::if_not_else)]

#[macro_use]
mod util;

mod addr;
mod aio;
mod ctx;
mod dialer;
mod error;
mod listener;
mod message;
mod pipe;
mod protocol;
mod socket;

pub mod options;

pub use crate::{
	addr::SocketAddr,
	aio::{Aio, AioResult},
	ctx::Context,
	dialer::{Dialer, DialerOptions},
	error::{Error, Result},
	listener::{Listener, ListenerOptions},
	message::{Header, Message},
	pipe::{Pipe, PipeEvent},
	protocol::Protocol,
	socket::Socket,
};
