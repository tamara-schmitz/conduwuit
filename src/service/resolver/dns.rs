use std::{iter, net::SocketAddr, sync::Arc, time::Duration};

use conduit::{err, Result, Server};
use futures::FutureExt;
use hickory_resolver::TokioAsyncResolver;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use super::cache::{Cache, CachedOverride};

pub struct Resolver {
	pub(crate) resolver: Arc<TokioAsyncResolver>,
	pub(crate) hooked: Arc<Hooked>,
}

pub(crate) struct Hooked {
	resolver: Arc<TokioAsyncResolver>,
	cache: Arc<Cache>,
}

type ResolvingResult = Result<Addrs, Box<dyn std::error::Error + Send + Sync>>;

impl Resolver {
	#[allow(clippy::as_conversions, clippy::cast_sign_loss, clippy::cast_possible_truncation)]
	pub(super) fn build(server: &Arc<Server>, cache: Arc<Cache>) -> Result<Arc<Self>> {
		let config = &server.config;
		let (sys_conf, mut opts) = hickory_resolver::system_conf::read_system_conf()
			.map_err(|e| err!(error!("Failed to configure DNS resolver from system: {e}")))?;

		let mut conf = hickory_resolver::config::ResolverConfig::new();

		if let Some(domain) = sys_conf.domain() {
			conf.set_domain(domain.clone());
		}

		for sys_conf in sys_conf.search() {
			conf.add_search(sys_conf.clone());
		}

		for sys_conf in sys_conf.name_servers() {
			let mut ns = sys_conf.clone();

			if config.query_over_tcp_only {
				ns.protocol = hickory_resolver::config::Protocol::Tcp;
			}

			ns.trust_negative_responses = !config.query_all_nameservers;

			conf.add_name_server(ns);
		}

		opts.cache_size = config.dns_cache_entries as usize;
		opts.negative_min_ttl = Some(Duration::from_secs(config.dns_min_ttl_nxdomain));
		opts.negative_max_ttl = Some(Duration::from_secs(60 * 60 * 24 * 30));
		opts.positive_min_ttl = Some(Duration::from_secs(config.dns_min_ttl));
		opts.positive_max_ttl = Some(Duration::from_secs(60 * 60 * 24 * 7));
		opts.timeout = Duration::from_secs(config.dns_timeout);
		opts.attempts = config.dns_attempts as usize;
		opts.try_tcp_on_error = config.dns_tcp_fallback;
		opts.num_concurrent_reqs = 1;
		opts.shuffle_dns_servers = true;
		opts.rotate = true;
		opts.ip_strategy = match config.ip_lookup_strategy {
			1 => hickory_resolver::config::LookupIpStrategy::Ipv4Only,
			2 => hickory_resolver::config::LookupIpStrategy::Ipv6Only,
			3 => hickory_resolver::config::LookupIpStrategy::Ipv4AndIpv6,
			4 => hickory_resolver::config::LookupIpStrategy::Ipv6thenIpv4,
			_ => hickory_resolver::config::LookupIpStrategy::Ipv4thenIpv6,
		};
		opts.authentic_data = false;

		let resolver = Arc::new(TokioAsyncResolver::tokio(conf, opts));
		Ok(Arc::new(Self {
			resolver: resolver.clone(),
			hooked: Arc::new(Hooked {
				resolver,
				cache,
			}),
		}))
	}
}

impl Resolve for Resolver {
	fn resolve(&self, name: Name) -> Resolving { resolve_to_reqwest(self.resolver.clone(), name).boxed() }
}

impl Resolve for Hooked {
	fn resolve(&self, name: Name) -> Resolving {
		let cached: Option<CachedOverride> = self
			.cache
			.overrides
			.read()
			.expect("locked for reading")
			.get(name.as_str())
			.cloned();

		cached.map_or_else(
			|| resolve_to_reqwest(self.resolver.clone(), name).boxed(),
			|cached| cached_to_reqwest(cached).boxed(),
		)
	}
}

async fn cached_to_reqwest(cached: CachedOverride) -> ResolvingResult {
	let first_ip = cached
		.ips
		.first()
		.expect("must provide at least one override");

	let saddr = SocketAddr::new(*first_ip, cached.port);

	Ok(Box::new(iter::once(saddr)))
}

async fn resolve_to_reqwest(resolver: Arc<TokioAsyncResolver>, name: Name) -> ResolvingResult {
	let results = resolver
		.lookup_ip(name.as_str())
		.await?
		.into_iter()
		.map(|ip| SocketAddr::new(ip, 0));

	Ok(Box::new(results))
}
