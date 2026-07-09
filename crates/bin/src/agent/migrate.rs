use super::setup::{SetupReport, Sys};
use crate::agent::migrate_ledger::LedgerHandle;
use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationState {
    Applicable,
    Done,
    NotApplicable,
}

pub struct MigrationOutcome {
    pub changed: bool,
    pub note: String,
}

#[async_trait]
pub trait Migration: Send + Sync {
    fn id(&self) -> &str;
    fn description(&self) -> &str;
    fn disruptive(&self) -> bool;
    async fn detect(&self, sys: &dyn Sys) -> MigrationState;
    async fn apply(&self, sys: &dyn Sys, dry_run: bool) -> Result<MigrationOutcome, String>;
}

/// Applies every non-disruptive Applicable migration and records it; for a
/// disruptive Applicable one, prints an actionable note instead of applying.
pub async fn run_auto(
    sys: &dyn Sys,
    ledger: &dyn LedgerHandle,
    registry: &[Box<dyn Migration>],
    dry: bool,
    rep: &mut SetupReport,
) {
    for m in registry {
        if ledger.is_applied(m.id()).await {
            continue;
        }
        match m.detect(sys).await {
            MigrationState::Done | MigrationState::NotApplicable => {}
            MigrationState::Applicable if m.disruptive() => {
                rep.warnings.push(format!(
                    "migration available: `{}` ({}). Run: sudo rpi agent migrate --run {}",
                    m.id(),
                    m.description(),
                    m.id()
                ));
            }
            MigrationState::Applicable => apply_one(sys, ledger, m.as_ref(), dry, rep).await,
        }
    }
}

/// Applies the named migrations regardless of `disruptive`.
pub async fn run_explicit(
    sys: &dyn Sys,
    ledger: &dyn LedgerHandle,
    registry: &[Box<dyn Migration>],
    ids: &[String],
    dry: bool,
    rep: &mut SetupReport,
) {
    for id in ids {
        match registry.iter().find(|m| m.id() == id) {
            None => rep.errors.push(format!("unknown migration: {id}")),
            Some(m) => match m.detect(sys).await {
                MigrationState::Done => rep.skipped.push(format!("migration {id} (already done)")),
                MigrationState::NotApplicable => {
                    rep.skipped.push(format!("migration {id} (not applicable)"))
                }
                MigrationState::Applicable => apply_one(sys, ledger, m.as_ref(), dry, rep).await,
            },
        }
    }
}

async fn apply_one(
    sys: &dyn Sys,
    ledger: &dyn LedgerHandle,
    m: &dyn Migration,
    dry: bool,
    rep: &mut SetupReport,
) {
    match m.apply(sys, dry).await {
        Ok(out) => {
            if !dry {
                ledger.mark_applied(m.id()).await;
            }
            rep.repaired
                .push(format!("migration {}: {}", m.id(), out.note));
        }
        Err(e) => rep.errors.push(format!("migration {} failed: {e}", m.id())),
    }
}

#[cfg(test)]
mod tests {
    use super::super::setup::fake::FakeSys;
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeLedger {
        applied: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl LedgerHandle for FakeLedger {
        async fn is_applied(&self, id: &str) -> bool {
            self.applied.lock().unwrap().iter().any(|a| a == id)
        }
        async fn mark_applied(&self, id: &str) {
            self.applied.lock().unwrap().push(id.to_string());
        }
    }

    struct Stub {
        id: &'static str,
        disruptive: bool,
        state: MigrationState,
    }
    #[async_trait]
    impl Migration for Stub {
        fn id(&self) -> &str {
            self.id
        }
        fn description(&self) -> &str {
            "stub"
        }
        fn disruptive(&self) -> bool {
            self.disruptive
        }
        async fn detect(&self, _sys: &dyn Sys) -> MigrationState {
            self.state
        }
        async fn apply(&self, _sys: &dyn Sys, _dry: bool) -> Result<MigrationOutcome, String> {
            Ok(MigrationOutcome {
                changed: true,
                note: "applied".into(),
            })
        }
    }

    #[tokio::test]
    async fn auto_applies_nondisruptive_and_records() {
        let reg: Vec<Box<dyn Migration>> = vec![Box::new(Stub {
            id: "safe",
            disruptive: false,
            state: MigrationState::Applicable,
        })];
        let ledger = FakeLedger::default();
        let mut rep = SetupReport::default();
        run_auto(&FakeSys::default(), &ledger, &reg, false, &mut rep).await;
        assert!(rep.repaired.iter().any(|r| r.contains("migration safe")));
        assert!(ledger.is_applied("safe").await);
    }

    #[tokio::test]
    async fn auto_only_reports_disruptive() {
        let reg: Vec<Box<dyn Migration>> = vec![Box::new(Stub {
            id: "nginx-to-caddy",
            disruptive: true,
            state: MigrationState::Applicable,
        })];
        let ledger = FakeLedger::default();
        let mut rep = SetupReport::default();
        run_auto(&FakeSys::default(), &ledger, &reg, false, &mut rep).await;
        assert!(rep
            .warnings
            .iter()
            .any(|w| w.contains("--run nginx-to-caddy")));
        assert!(
            !ledger.is_applied("nginx-to-caddy").await,
            "not auto-applied"
        );
    }

    #[tokio::test]
    async fn explicit_applies_disruptive() {
        let reg: Vec<Box<dyn Migration>> = vec![Box::new(Stub {
            id: "nginx-to-caddy",
            disruptive: true,
            state: MigrationState::Applicable,
        })];
        let ledger = FakeLedger::default();
        let mut rep = SetupReport::default();
        run_explicit(
            &FakeSys::default(),
            &ledger,
            &reg,
            &["nginx-to-caddy".to_string()],
            false,
            &mut rep,
        )
        .await;
        assert!(ledger.is_applied("nginx-to-caddy").await);
    }

    #[tokio::test]
    async fn explicit_unknown_is_error() {
        let reg: Vec<Box<dyn Migration>> = vec![];
        let ledger = FakeLedger::default();
        let mut rep = SetupReport::default();
        run_explicit(
            &FakeSys::default(),
            &ledger,
            &reg,
            &["nope".into()],
            false,
            &mut rep,
        )
        .await;
        assert!(rep.errors.iter().any(|e| e.contains("unknown migration")));
    }
}
