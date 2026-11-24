//! `alfred eval`: run a fixture of requests through the agent in dry run and measure what the design
//! promises: how much the router covers, and how often each path does the right thing.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use serde::Deserialize;

use crate::action::DryRun;
use crate::agent::{Agent, Event, Source};

#[derive(Debug, Clone, Deserialize)]
pub struct Case {
    pub text: String,
    /// The tool expected first, or "ask" for a follow-up question, or "chat" for words only.
    pub tool: String,
    /// Field → text the field must contain (case-insensitive).
    #[serde(default)]
    pub expect: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct Fixture {
    #[serde(rename = "case")]
    cases: Vec<Case>,
}

pub fn load(path: &Path) -> Result<Vec<Case>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Cases from fixture text (TOML with `[[case]]` tables).
pub fn parse(text: &str) -> Result<Vec<Case>, String> {
    toml::from_str::<Fixture>(text).map(|f| f.cases).map_err(|e| e.to_string())
}

#[derive(Debug, Clone)]
pub struct CaseResult {
    pub case: Case,
    pub source: Source,
    /// What Alfred did, as "tool {json}", or its reply when it did nothing.
    pub got: String,
    pub ok: bool,
    pub millis: u128,
    /// The model was asked (the router didn't answer).
    pub used_model: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub results: Vec<CaseResult>,
}

impl Report {
    fn count(&self, f: impl Fn(&CaseResult) -> bool) -> usize {
        self.results.iter().filter(|r| f(r)).count()
    }

    /// The router answered: no model call, and not the "no model" fallback.
    fn by_router(r: &CaseResult) -> bool {
        !r.used_model && r.source != Source::Error
    }

    /// Share of cases the router answered without the model.
    pub fn router_share(&self) -> f64 {
        ratio(self.count(Self::by_router), self.results.len())
    }

    pub fn accuracy(&self) -> f64 {
        ratio(self.count(|r| r.ok), self.results.len())
    }

    pub fn router_accuracy(&self) -> f64 {
        ratio(self.count(|r| Self::by_router(r) && r.ok), self.count(Self::by_router))
    }

    pub fn model_accuracy(&self) -> f64 {
        ratio(self.count(|r| !Self::by_router(r) && r.ok), self.count(|r| !Self::by_router(r)))
    }

    /// A plain-text table and summary.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for r in &self.results {
            let path = if Self::by_router(r) { "router" } else { "model " };
            out.push_str(&format!(
                "{} {path} {:>5} ms  {:<52} → {}\n",
                if r.ok { "✓" } else { "✗" },
                r.millis,
                truncate(&r.case.text, 52),
                truncate(&r.got, 90)
            ));
        }
        let model_n = self.count(|r| !Self::by_router(r));
        out.push_str(&format!(
            "\n{} cases · overall {:.0}% · router handled {:.0}% ({} of {}) at {:.0}% accuracy · model handled {} at {:.0}% accuracy\n",
            self.results.len(),
            self.accuracy() * 100.0,
            self.router_share() * 100.0,
            self.count(Self::by_router),
            self.results.len(),
            self.router_accuracy() * 100.0,
            model_n,
            self.model_accuracy() * 100.0,
        ));
        out
    }
}

fn ratio(a: usize, b: usize) -> f64 {
    if b == 0 { 0.0 } else { a as f64 / b as f64 }
}

fn truncate(s: &str, n: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= n { s } else { s.chars().take(n - 1).collect::<String>() + "…" }
}

/// Run every case with a fresh agent from `make_agent` and a dry-run executor.
pub fn run(cases: &[Case], mut make_agent: impl FnMut() -> Agent) -> Report {
    let mut report = Report::default();
    for case in cases {
        let mut agent = make_agent();
        let exec = DryRun::new();
        let started = Instant::now();
        let mut used_model = false;
        let turn = agent.handle(&case.text, &exec, &mut |e| used_model |= matches!(e, Event::Thinking));
        let millis = started.elapsed().as_millis();
        let first = exec.actions().into_iter().next().or(turn.pending.clone());
        let got = match &first {
            Some(a) => serde_json::to_string(a).unwrap_or_default(),
            None => format!("“{}”", turn.reply),
        };
        let ok = match case.tool.as_str() {
            "chat" => first.is_none() && !matches!(turn.source, Source::Error | Source::Clarify),
            "ask" => turn.source == Source::Clarify,
            tool => first.as_ref().is_some_and(|a| {
                let v = serde_json::to_value(a).unwrap_or_default();
                a.tool_name() == tool
                    && case.expect.iter().all(|(k, want)| {
                        let field = v.get(k).map(|f| f.as_str().map(str::to_string).unwrap_or_else(|| f.to_string())).unwrap_or_default();
                        field.to_lowercase().contains(&want.to_lowercase())
                    })
            }),
        };
        report.results.push(CaseResult { case: case.clone(), source: turn.source, got, ok, millis, used_model });
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Scripted;
    use serde_json::json;

    fn case(text: &str, tool: &str, expect: &[(&str, &str)]) -> Case {
        Case { text: text.into(), tool: tool.into(), expect: expect.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }
    }

    #[test]
    fn scores_router_and_model_paths_separately() {
        let cases = vec![
            case("open notes", "open_app", &[("name", "notes")]),
            case("search google for rune", "web_search", &[("query", "rune")]),
            case("I'd love some music", "open_app", &[("name", "music")]),
            case("hello there", "chat", &[]),
        ];
        let mut replies = vec![Scripted::calls("open_app", json!({"name": "Spotify"})), Scripted::says("Good evening.")].into_iter();
        let report = run(&cases, || Agent::new(Some(Box::new(Scripted::new(replies.next())))));
        assert_eq!(report.results.iter().map(|r| r.ok).collect::<Vec<_>>(), vec![true, true, false, true]);
        assert!((report.router_share() - 0.5).abs() < 1e-9);
        assert!((report.router_accuracy() - 1.0).abs() < 1e-9);
        assert!((report.model_accuracy() - 0.5).abs() < 1e-9);
        assert!(report.render().contains("router handled 50%"));
    }

    #[test]
    fn the_shipped_fixture_parses_and_the_router_gets_its_share_right() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../eval/utterances.toml");
        let cases = load(&path).unwrap();
        assert!(cases.len() >= 30, "a meaningful fixture");
        // Without a model, every router-shaped case must still be right.
        let report = run(&cases, || Agent::new(None));
        let wrong: Vec<_> = report.results.iter().filter(|r| Report::by_router(r) && !r.ok).map(|r| (&r.case.text, &r.got)).collect();
        assert!(wrong.is_empty(), "router mistakes: {wrong:#?}");
    }
}
