//! Review → fix loop on local work, before a PR exists. Nothing is committed,
//! pushed, or posted.

use std::fmt::Write;

use anyhow::{Result, bail};

use crate::agent::Scope;
use crate::comment::{self, Changes, FixOutcome, Item};
use crate::config::{Agent, Config};
use crate::finding::{FixReport, Status};
use crate::fix;
use crate::git;
use crate::review;

pub struct Options {
    /// Ref to compare against; `None` uses the repo's default branch.
    pub base: Option<String>,
    pub fix: bool,
    pub reviewers: Vec<Agent>,
    /// Review → fix rounds to run at most; `cr local` runs one.
    pub max_rounds: u32,
}

pub fn run(config: &Config, opts: Options) -> Result<()> {
    let base = match opts.base {
        Some(base) => base,
        None => git::default_base()?,
    };
    let scope = Scope::Local { base: &base };
    let out_dir = review::out_dir("local")?;
    // Findings the fixer rejected; later rounds tell the merger to drop them again.
    let mut rejected = String::new();
    let mut history = String::new();

    for round in 1..=opts.max_rounds {
        let before = git::changes_since(&base)?;
        if before.is_empty() {
            if round == 1 {
                bail!("no changes against {base} to review");
            }
            return Ok(());
        }
        if opts.max_rounds > 1 {
            eprintln!("Round {round}/{}", opts.max_rounds);
            writeln!(history, "# Round {round}/{}\n", opts.max_rounds)?;
        }

        let (findings, used) = review::collect(config, &opts.reviewers, scope, &out_dir, &rejected)?;
        let review_report = comment::render_findings(&findings, &used);
        println!("{review_report}");
        history.push_str(&review_report);
        review::open_report(&out_dir, &history);
        if findings.is_empty() || !opts.fix {
            return Ok(());
        }

        let (items, listing) = fix::list_findings(&findings);
        let report = fix::apply(config, scope, &items, &listing)?;
        let changes = if git::changes_since(&base)? == before { Changes::None } else { Changes::Uncommitted };
        let converged = matches!(changes, Changes::None);
        let outcome = FixOutcome { items: &items, report: &report, changes };
        let fix_report = outcome.render();
        println!("{fix_report}");
        writeln!(history, "\n{fix_report}")?;
        review::open_report(&out_dir, &history);
        if !outcome.is_green() {
            bail!("not everything is green; see the summary above");
        }
        // Nothing changed: the fixer judged every finding invalid.
        if converged {
            return Ok(());
        }
        rejected.push_str(&rejected_listing(&items, &report));
    }
    if opts.max_rounds > 1 {
        bail!("reached --max-rounds {}; the last round's fixes were not re-reviewed", opts.max_rounds);
    }
    Ok(())
}

/// One markdown line per finding the fixer marked invalid, with its reason.
fn rejected_listing(items: &[Item], report: &FixReport) -> String {
    let mut out = String::new();
    for r in report.results.iter().filter(|r| r.status == Status::Invalid) {
        if let Some(item) = items.iter().find(|i| i.id == r.id) {
            writeln!(out, "- {}: {}", item.title, r.note).unwrap();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::finding::FixResult;

    #[test]
    fn lists_only_invalid_findings() {
        let item = |id: &str, title: &str| Item { id: id.into(), title: title.into() };
        let result = |id: &str, status, note: &str| FixResult { id: id.into(), status, note: note.into() };
        let report = FixReport {
            results: vec![result("F1", Status::Fixed, "done"), result("F2", Status::Invalid, "already guarded")],
            checks: vec![],
        };
        let listing = rejected_listing(&[item("F1", "Off by one"), item("F2", "Null deref")], &report);
        assert_eq!(listing, "- Null deref: already guarded\n");
    }
}
