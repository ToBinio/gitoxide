#![allow(clippy::result_large_err)]
use std::convert::TryInto;

use crate::{bstr::BString, config::tree::gitoxide, remote};

type ConfigureRemoteFn =
    Box<dyn FnMut(crate::Remote<'_>) -> Result<crate::Remote<'_>, Box<dyn std::error::Error + Send + Sync>>>;
#[cfg(any(feature = "async-network-client", feature = "blocking-network-client"))]
type ConfigureConnectionFn = Box<
    dyn FnMut(
        &mut remote::Connection<'_, '_, Box<dyn gix_protocol::transport::client::Transport + Send>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>,
>;

/// A utility to collect configuration on how to fetch from a remote and initiate a fetch operation. It will delete the newly
/// created repository on when dropped without successfully finishing a fetch.
#[must_use]
pub struct PrepareFetch {
    /// A freshly initialized repository which is owned by us, or `None` if it was handed to the user
    repo: Option<crate::Repository>,
    /// The name of the remote, which defaults to `origin` if not overridden.
    remote_name: Option<BString>,
    /// A function to configure a remote prior to fetching a pack.
    configure_remote: Option<ConfigureRemoteFn>,
    /// A function to configure a connection before using it.
    #[cfg(any(feature = "async-network-client", feature = "blocking-network-client"))]
    configure_connection: Option<ConfigureConnectionFn>,
    /// Options for preparing a fetch operation.
    #[cfg(any(feature = "async-network-client", feature = "blocking-network-client"))]
    fetch_options: remote::ref_map::Options,
    /// The url to clone from
    #[cfg_attr(not(feature = "blocking-network-client"), allow(dead_code))]
    url: gix_url::Url,
    /// How to handle shallow clones
    #[cfg_attr(not(feature = "blocking-network-client"), allow(dead_code))]
    shallow: remote::fetch::Shallow,
}

/// The error returned by [`PrepareFetch::new()`].
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
    #[error(transparent)]
    Init(#[from] crate::init::Error),
    #[error(transparent)]
    UrlParse(#[from] gix_url::parse::Error),
    #[error("Failed to turn a the relative file url \"{}\" into an absolute one", url.to_bstring())]
    CanonicalizeUrl {
        url: gix_url::Url,
        source: gix_path::realpath::Error,
    },
}

/// Instantiation
impl PrepareFetch {
    /// Create a new repository at `path` with `crate_opts` which is ready to clone from `url`, possibly after making additional adjustments to
    /// configuration and settings.
    ///
    /// Note that this is merely a handle to perform the actual connection to the remote, and if any of it fails the freshly initialized repository
    /// will be removed automatically as soon as this instance drops.
    ///
    /// # Deviation
    ///
    /// Similar to `git`, a missing user name and email configuration is not terminal and we will fill it in with dummy values. However,
    /// instead of deriving values from the system, ours are hardcoded to indicate what happened.
    #[allow(clippy::result_large_err)]
    pub fn new<Url, E>(
        url: Url,
        path: impl AsRef<std::path::Path>,
        kind: crate::create::Kind,
        mut create_opts: crate::create::Options,
        open_opts: crate::open::Options,
    ) -> Result<Self, Error>
    where
        Url: TryInto<gix_url::Url, Error = E>,
        gix_url::parse::Error: From<E>,
    {
        let mut url = url.try_into().map_err(gix_url::parse::Error::from)?;
        url.canonicalize().map_err(|err| Error::CanonicalizeUrl {
            url: url.clone(),
            source: err,
        })?;
        create_opts.destination_must_be_empty = true;
        let mut repo = crate::ThreadSafeRepository::init_opts(path, kind, create_opts, open_opts)?.to_thread_local();
        if repo.committer().is_none() {
            let mut config = gix_config::File::new(gix_config::file::Metadata::api());
            config
                .set_raw_value(
                    "gitoxide",
                    Some("committer".into()),
                    gitoxide::Committer::NAME_FALLBACK.name,
                    "no name configured during clone",
                )
                .expect("works - statically known");
            config
                .set_raw_value(
                    "gitoxide",
                    Some("committer".into()),
                    gitoxide::Committer::EMAIL_FALLBACK.name,
                    "noEmailAvailable@example.com",
                )
                .expect("works - statically known");
            let mut repo_config = repo.config_snapshot_mut();
            repo_config.append(config);
            repo_config.commit().expect("configuration is still valid");
        }
        Ok(PrepareFetch {
            url,
            #[cfg(any(feature = "async-network-client", feature = "blocking-network-client"))]
            fetch_options: Default::default(),
            repo: Some(repo),
            remote_name: None,
            configure_remote: None,
            #[cfg(any(feature = "async-network-client", feature = "blocking-network-client"))]
            configure_connection: None,
            shallow: remote::fetch::Shallow::NoChange,
        })
    }
}

/// A utility to collect configuration on how to perform a checkout into a working tree, and when dropped without checking out successfully
/// the fetched repository will be dropped.
#[must_use]
pub struct PrepareCheckout {
    /// A freshly initialized repository which is owned by us, or `None` if it was handed to the user
    pub(self) repo: Option<crate::Repository>,
}

mod access;

// This module encapsulates functionality that works with both feature toggles. Can be combined with `fetch`
// once async and clone are a thing.
#[cfg(any(feature = "async-network-client", feature = "blocking-network-client"))]
mod access_feat {
    use crate::clone::PrepareFetch;

    /// Builder
    impl PrepareFetch {
        /// Set a callback to use for configuring the connection to use right before connecting to the remote.
        ///
        /// It is most commonly used for custom configuration.
        // TODO: tests
        pub fn configure_connection(
            mut self,
            f: impl FnMut(
                    &mut crate::remote::Connection<'_, '_, Box<dyn gix_protocol::transport::client::Transport + Send>>,
                ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
                + 'static,
        ) -> Self {
            self.configure_connection = Some(Box::new(f));
            self
        }

        /// Set additional options to adjust parts of the fetch operation that are not affected by the git configuration.
        pub fn with_fetch_options(mut self, opts: crate::remote::ref_map::Options) -> Self {
            self.fetch_options = opts;
            self
        }
    }
}

///
#[cfg(any(feature = "async-network-client-async-std", feature = "blocking-network-client"))]
pub mod fetch;

///
pub mod checkout;
