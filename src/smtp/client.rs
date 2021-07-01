use std::{net::IpAddr, time::Duration};
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	net::TcpStream,
	time::{error::Elapsed, timeout},
};

use super::{
	args::{ForwardPath, Path, ReversePath},
	Command, ResponseCode,
};

use thiserror::Error;

/// A small wrapper around Path as a type-checked, compile-time feature to try
// and stop us from doing stupid things and trying to relay local messages.
#[derive(Debug, Clone)]
pub struct ForeignPath(pub Path);

impl From<ForeignPath> for ForwardPath {
	fn from(other: ForeignPath) -> Self {
		Self::Regular(other.0)
	}
}

#[derive(Debug, Clone)]
pub struct ForeignMessage {
	pub reverse_path: ReversePath,
	pub forward_paths: Vec<ForeignPath>,
	pub data: Vec<String>,
}

impl ForeignMessage {
	pub fn from_parts(
		reverse_path: ReversePath,
		forward_paths: Vec<ForeignPath>,
		data: Vec<String>,
	) -> Self {
		Self {
			reverse_path,
			forward_paths,
			data,
		}
	}
}

impl Default for ForeignMessage {
	fn default() -> Self {
		Self {
			reverse_path: ReversePath::Null,
			forward_paths: vec![],
			data: vec![],
		}
	}
}

#[derive(Default, Clone)]
pub struct Client {
	state: State,
	reply: String,
	message: ForeignMessage,
}

impl Client {
	pub fn initiate(message: ForeignMessage) -> Self {
		Self {
			message,
			..Default::default()
		}
	}

	pub fn push(&mut self, reply: &str) -> Option<Command> {
		self.reply.push_str(reply);

		if !self.reply.ends_with("\r\n") {
			return None;
		}

		//todo: process shouldExit and sendingData state variants

		self.process_reply()
	}

	fn process_reply(&mut self) -> Option<Command> {
		if self.reply.len() < 3 || !self.reply.is_ascii() {
			return None;
		}
		let code = self.reply.split_at(3).0;

		//todo: parse multiline replies e.g. ehlo
		//todo: handle the unknown response codes
		let code = ResponseCode::from_code(code.parse().ok()?)?;

		match self.state {
			State::Initiated => match code {
				ResponseCode::ServiceReady => {
					self.state = State::Greeted;
					Some(Command::Ehlo("Sail".parse().unwrap())) //todo: use actual hostname, not Sail
				}
				_ => todo!(),
			},
			State::Greeted => match code {
				ResponseCode::Okay => {
					self.state = State::SentReversePath;
					Some(Command::Mail(self.message.reverse_path.clone()))
				}
				_ => todo!(),
			},
			State::SentReversePath => match code {
				ResponseCode::Okay => {
					self.state = State::SendingForwardPaths;
					Some(Command::Rcpt(self.message.forward_paths.pop()?.into()))
				}
				_ => todo!(),
			},
			State::SendingForwardPaths => {
				if let Some(path) = self.message.forward_paths.pop() {
					match code {
						ResponseCode::Okay | ResponseCode::UserNotLocalWillForward => {
							Some(Command::Rcpt(path.into()))
						}
						_ => todo!(),
					}
				} else {
					match code {
						ResponseCode::Okay | ResponseCode::UserNotLocalWillForward => {
							self.state = State::SendingData;
							Some(Command::Data)
						}
						_ => todo!(),
					}
				}
			}
			State::SendingData => unreachable!(),
			State::ShouldExit => unreachable!(),
		}
	}

	pub async fn run(address: IpAddr, message: ForeignMessage) -> Result<(), ClientError> {
		let domain = message
			.forward_paths
			.first()
			.ok_or(ClientError::NoForwardPaths)?
			.0
			.domain
			.clone();

		for path in &message.forward_paths {
			if path.0.domain != domain {
				return Err(ClientError::MismatchedDomains);
			}
		}

		Self::send_to_ip(address, message).await.unwrap(); //TODO: handle these results and inform user about them

		todo!() //TODO: send 250 if the message sent properly, otherwise a 5xx error or whatever the remote server sent
		 //alternatively, send 250 immediately, then construct an undeliverable message
	}

	async fn send_to_ip(addr: IpAddr, message: ForeignMessage) -> Result<(), ClientError> {
		//TODO: use our own errors? send box dyn error?
		eprintln!("{}:{}", addr, 25);
		//todo: this one hangs interminably. why? i do not know
		//todo: timeouts.
		//todo: send failed connection message if port 25 is blocked, or something
		let mut stream = timeout(
			Duration::from_millis(2500),
			TcpStream::connect(format!("{}:{}", addr, 25)),
		)
		.await??;

		let mut client = Self::initiate(message);

		let mut buf = vec![0; 1024];

		while !client.should_exit() {
			let read = stream.read(&mut buf).await.unwrap();

			// A zero sized read, this connection has died or been terminated by the server
			if read == 0 {
				println!("Connection unexpectedly closed by server");
				return Ok(());
			}
			if client.state == State::SendingData
				&& buf.ends_with("\r\n".as_bytes())
				&& buf.starts_with("354".as_bytes())
			{
				//todo: transparency? leading .?
				for line in &client.message.data {
					stream.write_all(line.as_bytes()).await.unwrap();
					stream.write_all("\r\n".as_bytes()).await.unwrap()
				}
				stream.write_all(".\r\n".as_bytes()).await.unwrap();

				let read = stream.read(&mut buf).await.unwrap();
				if read == 0 {
					panic!("oh no")
				} else if buf.starts_with("250".as_bytes()) && buf.ends_with("\r\n".as_bytes()) {
					return Ok(());
				}
			}

			let command = client.push(String::from_utf8_lossy(&buf[..read]).as_ref());

			if let Some(command) = command {
				stream.write_all(command.to_string().as_bytes()).await?;
			}
		}
		Ok(())
	}

	fn should_exit(&self) -> bool {
		self.state == State::ShouldExit
	}
}

#[derive(Clone, Copy, PartialEq)]
enum State {
	Initiated,
	Greeted,
	SentReversePath,
	SendingForwardPaths,
	SendingData,
	ShouldExit,
}

impl Default for State {
	fn default() -> Self {
		State::Initiated
	}
}

#[derive(Debug, Error)]
pub enum ClientError {
	#[error("there are no forward paths in the provided message")]
	NoForwardPaths,
	#[error("there were forward paths with more than one domain")]
	MismatchedDomains,
	#[error("timed out before reaching the server")]
	ConnectionTimeout(#[from] Elapsed),
	#[error("")]
	ConnectionError(#[from] std::io::Error),
}
