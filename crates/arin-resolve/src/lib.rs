//! Grounding resolvers.
//!
//! A resolver turns `"the Submit button"` into coordinates. The `Resolver` trait is
//! defined in `arin-core` alongside the other platform seams. This crate holds the
//! registry that maps configuration names to implementations, plus the adapters
//! themselves.
//!
//! # Status
//!
//! Two adapters. [`ClaudeResolver`] grounds against a hosted model with the user's own API
//! key. [`LocalResolver`] grounds against a model served on this machine, which is what
//! removes both the key and the screenshot egress.
//!
//! They share everything except how they reach a model: the instructions, the schema, the
//! range checks, and the one conversion out of image pixels all live in [`grounding`]. Two
//! adapters quietly disagreeing about which corner a coordinate is measured from is the
//! failure that module exists to prevent.
//!
//! # Why the registry holds builders rather than resolvers
//!
//! Configuration names one resolver, and constructing any of them can fail: a missing
//! key, an unreadable model file, a socket that is not there. A registry of live
//! instances would have to build every adapter it knows about in order to offer a choice
//! between them, so a user who configured a local model would still need an API key for
//! the hosted one they did not ask for. Builders defer that to the one that was chosen.
//!
//! # Egress
//!
//! A resolver is the only part of Arin that can send anything off the machine. Every
//! implementation must report [`arin_core::Resolver::is_remote`] honestly, because that
//! is what the consent prompt is built on. A resolver that quietly returns `false` while
//! making a network call defeats the entire privacy story.
//!
//! Nothing here is enabled by default, and no adapter is selected by inference. A key in
//! the environment is not consent, so the daemon grounds nothing until it is told which
//! resolver to use by name.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod claude;
pub mod eval;
pub mod grounding;
pub mod local;
pub mod screenshot;

pub use claude::ClaudeResolver;
pub use local::LocalResolver;

use arin_core::{Error, Resolver, Result};
use std::collections::HashMap;
use std::sync::Arc;

/// Constructs a resolver, or explains why it cannot.
pub type Builder = Box<dyn Fn() -> Result<Arc<dyn Resolver>> + Send + Sync>;

/// Maps configured names to the adapters that can be built under them.
///
/// The daemon holds at most one live resolver. The registry exists so adding a community
/// adapter is a registration call rather than a patch to core.
#[derive(Default)]
pub struct Registry {
    builders: HashMap<String, Builder>,
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every adapter that ships with Arin.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        registry.register("claude", || {
            Ok(Arc::new(ClaudeResolver::from_env()?) as Arc<dyn Resolver>)
        });
        registry.register("local", || {
            Ok(Arc::new(LocalResolver::from_env()?) as Arc<dyn Resolver>)
        });
        registry
    }

    /// Register a way to build a resolver.
    ///
    /// Returns whether this replaced an existing registration under the same name.
    pub fn register<F>(&mut self, name: impl Into<String>, build: F) -> bool
    where
        F: Fn() -> Result<Arc<dyn Resolver>> + Send + Sync + 'static,
    {
        let name = name.into();
        tracing::debug!(%name, "registered resolver");
        self.builders.insert(name, Box::new(build)).is_some()
    }

    /// Build the resolver registered under a name.
    ///
    /// An unknown name lists what is available rather than failing bare. This is read off
    /// a config file or a command line, so a typo is the likeliest cause and the set of
    /// right answers is short.
    pub fn build(&self, name: &str) -> Result<Arc<dyn Resolver>> {
        let build = self.builders.get(name).ok_or_else(|| {
            let known = self.names();
            Error::Resolver(if known.is_empty() {
                format!("no resolver named {name:?}, and none are registered")
            } else {
                format!(
                    "no resolver named {name:?}. Available: {}",
                    known.join(", ")
                )
            })
        })?;
        build()
    }

    /// Whether a name is registered, without building it.
    pub fn contains(&self, name: &str) -> bool {
        self.builders.contains_key(name)
    }

    /// Every registered name, sorted.
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.builders.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }

    /// Whether any resolver is registered.
    pub fn is_empty(&self) -> bool {
        self.builders.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arin_core::{Frame, Resolution};
    use futures::future::BoxFuture;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Stub(&'static str, bool);

    impl Resolver for Stub {
        fn name(&self) -> &str {
            self.0
        }

        fn is_remote(&self) -> bool {
            self.1
        }

        fn resolve<'a>(
            &'a self,
            _query: &'a str,
            _frame: &'a Frame,
        ) -> BoxFuture<'a, Result<Resolution>> {
            Box::pin(async { unreachable!("stub resolver is never invoked") })
        }
    }

    fn stub(name: &'static str, remote: bool) -> Result<Arc<dyn Resolver>> {
        Ok(Arc::new(Stub(name, remote)) as Arc<dyn Resolver>)
    }

    #[test]
    fn a_registered_name_builds() {
        let mut registry = Registry::new();
        assert!(registry.is_empty());

        registry.register("local-cua", || stub("local-cua", false));
        registry.register("hosted-cua", || stub("hosted-cua", true));

        assert_eq!(registry.names(), vec!["hosted-cua", "local-cua"]);
        assert!(registry.contains("local-cua"));
        assert!(!registry.build("local-cua").unwrap().is_remote());
        assert!(registry.build("hosted-cua").unwrap().is_remote());
    }

    #[test]
    fn re_registering_replaces() {
        let mut registry = Registry::new();
        assert!(!registry.register("cua", || stub("cua", false)));
        assert!(registry.register("cua", || stub("cua", true)));

        assert_eq!(registry.names().len(), 1);
        assert!(registry.build("cua").unwrap().is_remote());
    }

    /// A typo in a config file is the likeliest reason to land here, so the error is
    /// worth more than "not found".
    #[test]
    fn an_unknown_name_lists_what_there_is() {
        let mut registry = Registry::new();
        registry.register("claude", || stub("claude", true));

        let error = registry
            .build("cluade")
            .err()
            .expect("a typo is refused")
            .to_string();
        assert!(error.contains("cluade"), "got {error}");
        assert!(
            error.contains("claude"),
            "the near miss is the useful part, got {error}"
        );
    }

    #[test]
    fn an_empty_registry_says_so_rather_than_listing_nothing() {
        let error = Registry::new()
            .build("claude")
            .err()
            .expect("an empty registry builds nothing")
            .to_string();
        assert!(error.contains("none are registered"), "got {error}");
    }

    /// The reason the registry holds builders at all. Naming one resolver must not
    /// construct the others, because constructing them can fail for reasons the user has
    /// no interest in fixing.
    #[test]
    fn only_the_chosen_resolver_is_built() {
        static BUILT: AtomicUsize = AtomicUsize::new(0);

        let mut registry = Registry::new();
        registry.register("wanted", || stub("wanted", false));
        registry.register("unwanted", || {
            BUILT.fetch_add(1, Ordering::Relaxed);
            Err(Error::Resolver("needs a key nobody supplied".into()))
        });

        assert!(registry.build("wanted").is_ok());
        assert_eq!(
            BUILT.load(Ordering::Relaxed),
            0,
            "registering an adapter must not construct it"
        );
    }

    /// A failure to build is the adapter's own message, not a generic one. This is where
    /// "you have not set an API key" reaches the user.
    #[test]
    fn a_builder_that_fails_says_why() {
        let mut registry = Registry::new();
        registry.register("claude", || {
            Err(Error::Resolver("ANTHROPIC_API_KEY is not set".into()))
        });

        let error = registry
            .build("claude")
            .err()
            .expect("the builder fails")
            .to_string();
        assert!(error.contains("ANTHROPIC_API_KEY"), "got {error}");
    }

    #[test]
    fn the_builtins_are_registered_without_being_built() {
        let registry = Registry::with_builtins();
        assert!(registry.contains("claude"));
        assert!(registry.contains("local"));
        assert_eq!(registry.names(), vec!["claude", "local"]);
    }
}
