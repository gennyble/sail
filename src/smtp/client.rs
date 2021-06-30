#[derive(Default, Clone)]
pub struct Client {
	state: State,
	reply: String,
	message: Message,
}

use std::{collections::HashSet, net::IpAddr, time::Duration};
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	net::TcpStream,
	time::timeout,
};
use trust_dns_resolver::{
	config::{ResolverConfig, ResolverOpts},
	TokioAsyncResolver,
};

use super::{
	args::{Domain, ForwardPath, Path, ReversePath},
	Command, Message, ResponseCode,
};

impl Client {
	pub fn initiate(
		forward_paths: Vec<ForwardPath>, //todo: make this paths? handle postmaster higher up?
		reverse_path: ReversePath,
		data: Vec<String>,
	) -> Self {
		Self {
			message: Message {
				reverse_path,
				forward_paths,
				data,
			},
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
		let (code, text) = self.reply.split_at(3);

		//todo: parse multiline replies e.g. ehlo
		//todo: parse unknown response codes according to their first digit
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
					Some(Command::Rcpt(self.message.forward_paths.pop()?))
				}
				_ => todo!(),
			},
			State::SendingForwardPaths => {
				if let Some(path) = self.message.forward_paths.pop() {
					match code {
						ResponseCode::Okay | ResponseCode::UserNotLocalWillForward => {
							Some(Command::Rcpt(path))
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

	async fn get_mx_record(fqdn: &str) -> Option<IpAddr> {
		let resolver =
			TokioAsyncResolver::tokio(ResolverConfig::default(), ResolverOpts::default()).ok()?;
		let mut mx_rec: Vec<(u16, String)> = resolver
			.mx_lookup(fqdn)
			.await
			.ok()?
			.iter()
			.map(|mx| (mx.preference(), mx.exchange().to_string()))
			.collect();
		mx_rec.sort_by(|(pref1, _), (pref2, _)| pref1.cmp(pref2));
		let mx_domain = mx_rec.first()?.1.clone();
		let mx_ip = resolver.lookup_ip(mx_domain).await.ok()?;
		mx_ip.iter().next()
	}

	async fn get_ip(fqdn: &str) -> Option<IpAddr> {
		let resolver =
			TokioAsyncResolver::tokio(ResolverConfig::default(), ResolverOpts::default()).ok()?;

		let ip = resolver.lookup_ip(fqdn).await.ok()?;
		ip.iter().next()
	}

	fn postmaster(message: Message) {
		todo!() //save to disk? what to do with postmaster stuff
	}

	pub async fn run(message: Message) {
		let domains: HashSet<&Domain> = message
			.forward_paths
			.iter()
			.filter_map(|path| match path {
				ForwardPath::Postmaster => {
					Self::postmaster(message.clone());
					None
				}
				ForwardPath::Regular(path) => Some(&path.domain),
			}) //map paths to the second half of the string
			.collect();

		let mut paths_by_domain: Vec<(&Domain, Vec<&Path>)> = vec![];

		for domain in domains {
			paths_by_domain.push((
				domain,
				message
					.forward_paths
					.iter()
					.filter_map(|path| match path {
						ForwardPath::Postmaster => None,
						ForwardPath::Regular(path) => {
							if path.domain == *domain {
								Some(path)
							} else {
								None
							}
						}
					}) //filter for paths to the current domain
					.collect(),
			))
		}

		for (domain, paths) in paths_by_domain {
			let address = match domain {
				Domain::Literal(ip) => ip.to_owned(),
				Domain::FQDN(domain) => {
					if let Some(address) = Self::get_mx_record(domain).await {
						address
					} else if let Some(address) = Self::get_ip(domain).await {
						address
					} else {
						eprintln!("No record at all found for domain {}", domain);
						todo!() // this needs to be properly handled.
					}
				}
			};

			Self::send_to_ip(
				address,
				paths,
				message.reverse_path.clone(),
				message.data.clone(),
			)
			.await
			.unwrap(); //TODO: handle these results and inform user about them
		}

		todo!() //TODO: send 250 if the message sent properly, otherwise a 5xx error or whatever the remote server sent
		 //alternatively, send 250 immediately, then construct an undeliverable message
	}
	async fn send_to_ip(
		addr: IpAddr,
		paths: Vec<&Path>,
		reverse_path: ReversePath,
		data: Vec<String>,
	) -> std::io::Result<()> {
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

		let mut client = Self::initiate(
			paths
				.into_iter()
				.map(|path| ForwardPath::Regular(path.clone()))
				.collect(),
			reverse_path,
			data,
		);

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