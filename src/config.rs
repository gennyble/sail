use crate::smtp::args::{Domain, ForwardPath, LocalPart, Path};

#[derive(Clone)]
pub struct Config {
	//TODO: Properly load a config and don't have this be public!
	pub hostnames: Vec<Domain>,
	pub relays: Vec<Domain>,
	pub users: Vec<LocalPart>,
}

impl Config {
	pub fn forward_path_is_local(&self, forward: &ForwardPath) -> bool {
		match forward {
			ForwardPath::Postmaster => true,
			ForwardPath::Regular(path) => self.path_is_local(path),
		}
	}

	fn path_is_local(&self, path: &Path) -> bool {
		self.hostnames.contains(&path.domain)
	}
}
