//! A simple REQ/REP demonstration application.
//!
//! This is derived from the `nng` demonstration program which in turn was
//! derived from the legacy nanomsg demonstration program. The program
//! implements a simple RPC style service, which just returns the number of
//! seconds since the Unix epoch.
extern crate nng;
extern crate byteorder;

use std::{env, mem, process};
use std::time::SystemTime;
use nng::{Socket, Protocol};
use byteorder::{ByteOrder, LittleEndian};

/// Message representing a date request
const DATE_REQUEST: u64 = 1;

/// Entry point of the application
fn main()
{
	// Begin by parsing the arguments to gather whether this is the client or
	// the server and what URL to connect with.
	let args: Vec<_> = env::args().take(3).collect();

	match &args[..] {
		[_, t, url] if t == "client" => client(url),
		[_, t, url] if t == "server" => server(url),
		_ => {
			println!("Usage: reqrep client|server <URL>");
			process::exit(1);
		}
	}
}

/// Run the client portion of the program.
fn client(url: &str)
{
	let mut s = Socket::new(Protocol::Req0).unwrap();
	s.dial(url, false).unwrap();

	println!("CLIENT: SENDING DATE REQUEST");
	let mut req = [0u8; mem::size_of::<u64>()];
	LittleEndian::write_u64(&mut req, DATE_REQUEST);
	s.send_buf(&req, false).unwrap();

	println!("CLIENT: WAITING FOR RESPONSE");
	let res = s.recv(false).unwrap();
	let epoch = LittleEndian::read_u64(&res);

	println!("CLIENT: UNIX EPOCH WAS {} SECONDS AGO", epoch);
}

/// Run the server portion of the program.
fn server(url: &str)
{
	let mut s = Socket::new(Protocol::Rep0).unwrap();
	s.listen(url, false).unwrap();

	loop {
		println!("SERVER: WAITING FOR COMMAND");
		let mut msg = s.recv(false).unwrap();

		println!("SERVER: RECEIVED DATE REQUEST");
		let rep = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
		LittleEndian::write_u64(&mut msg, rep);

		println!("SERVER: SENDING {}", rep);
		s.send(msg, false).unwrap();
	}
}
