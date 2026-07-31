//! Score a resolver against a labelled corpus of real screens.
//!
//! The eval set owed since 0.3. Two things come out of it: whether grounding is accurate
//! enough to rely on, and whether the confidence a model reports predicts that, which is
//! the only evidence that can move `arin_core::policy::HIGH_CONFIDENCE` off the 0.85 it was
//! guessed at.
//!
//! Build a corpus:
//!
//! ```text
//! arin capture --save corpus                 # repeat, on whatever screens matter
//! ```
//!
//! Each capture writes a PNG next to the raw pixels. Open it, read off where each target
//! is in image pixels, and write `corpus/cases.json`:
//!
//! ```json
//! {
//!   "name": "laptop, editor and browser, 2026-07-31",
//!   "cases": [
//!     {
//!       "frame": "000-display1",
//!       "query": "the Submit button",
//!       "target": [820, 176, 96, 32]
//!     }
//!   ]
//! }
//! ```
//!
//! Then run it:
//!
//! ```text
//! cargo run --release -p arin-resolve --example eval -- corpus local
//! ANTHROPIC_API_KEY=... cargo run --release -p arin-resolve --example eval -- corpus claude
//! ```
//!
//! Labelling is by hand and there is no way around that. A target nobody wrote down is not
//! ground truth, and a corpus labelled by the thing being measured would only prove that
//! the model agrees with itself.
//!
//! The corpus is raw captures of somebody's screen, so it belongs outside the repo. Nothing
//! here writes into the tree by default and nothing uploads anything: `claude` sends each
//! frame to the API as part of resolving, which is the same egress that resolver always
//! has, and `local` sends nothing anywhere.

use arin_resolve::eval::{self, Report};
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(dir), Some(name)) = (args.next(), args.next()) else {
        eprintln!(
            "usage: eval <corpus directory> <resolver name>\n\n\
             Build a corpus with `arin capture --save <dir>`, label it in <dir>/cases.json,\n\
             then score a resolver against it. See the module docs for the format."
        );
        std::process::exit(2);
    };
    let dir = PathBuf::from(dir);

    let cases = match eval::load(&dir) {
        Ok(cases) if cases.is_empty() => {
            eprintln!("{} has no cases in it", dir.display());
            std::process::exit(1);
        }
        Ok(cases) => cases,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let resolver = match arin_resolve::Registry::with_builtins().build(&name) {
        Ok(resolver) => resolver,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    if resolver.is_remote() {
        eprintln!(
            "{name} sends every frame in this corpus off the machine. {} cases, one \
             screenshot each.",
            cases.len()
        );
    }
    eprintln!("scoring {} cases against {name}...", cases.len());

    // One at a time rather than concurrently, on purpose. Latency is one of the numbers
    // being measured and a local model serving several requests at once reports a number
    // nobody will ever see in use, since the daemon resolves one query at a time.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let outcomes = runtime.block_on(eval::run(resolver.as_ref(), &cases));

    let corpus_name = std::fs::read_to_string(dir.join("cases.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<eval::Corpus>(&text).ok())
        .map(|corpus| corpus.name)
        .unwrap_or_else(|| dir.display().to_string());

    print!(
        "{}",
        Report {
            corpus: corpus_name,
            resolver: resolver.name().to_owned(),
            remote: resolver.is_remote(),
            outcomes,
        }
        .render()
    );
}
