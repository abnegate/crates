use reqwest::dns::Addrs;
use reqwest::dns::Name;
use reqwest::dns::Resolve;
use reqwest::dns::Resolving;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;

/// A resolver that writes down every name it is asked for and then asks the
/// operating system, so a test can tell which resolver a client consults.
#[derive(Debug, Clone, Default)]
pub(super) struct Recording {
    names: Arc<Mutex<Vec<String>>>,
}

impl Recording {
    /// Every name asked for so far, in the order it was asked for.
    pub(super) fn names(&self) -> Vec<String> {
        self.names.lock().unwrap().clone()
    }
}

impl Resolve for Recording {
    fn resolve(&self, name: Name) -> Resolving {
        let names = Arc::clone(&self.names);
        Box::pin(async move {
            names.lock().unwrap().push(name.as_str().to_string());
            let addresses: Vec<SocketAddr> =
                tokio::net::lookup_host((name.as_str(), 0)).await?.collect();
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}
