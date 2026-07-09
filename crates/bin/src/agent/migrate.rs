use super::setup;
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

/// Rename a legacy `pi-agent` install to `rpi-agent` in place (user, group,
/// /var/lib, /etc, /var/log). Non-disruptive: no operator confirmation needed
/// since it only affects a legacy install path, never a fresh one.
pub struct PiToRpi;

#[async_trait]
impl Migration for PiToRpi {
    fn id(&self) -> &str {
        "pi-to-rpi"
    }
    fn description(&self) -> &str {
        "rename legacy pi-agent install to rpi-agent (user, group, /var/lib, /etc, /var/log)"
    }
    fn disruptive(&self) -> bool {
        false
    }
    async fn detect(&self, sys: &dyn Sys) -> MigrationState {
        if setup::user_exists(sys, "rpi-agent").await {
            MigrationState::NotApplicable // already migrated / fresh
        } else if setup::user_exists(sys, "pi-agent").await {
            MigrationState::Applicable
        } else {
            MigrationState::NotApplicable
        }
    }
    async fn apply(&self, sys: &dyn Sys, dry_run: bool) -> Result<MigrationOutcome, String> {
        let mut rep = SetupReport::default();
        setup::migrate_pi_agent(sys, dry_run, &mut rep).await;
        if let Some(e) = rep.errors.first() {
            return Err(e.clone());
        }
        Ok(MigrationOutcome {
            changed: true,
            note: "pi-agent -> rpi-agent".into(),
        })
    }
}

pub fn registry() -> Vec<Box<dyn Migration>> {
    vec![Box::new(PiToRpi)]
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

    #[tokio::test]
    async fn dry_run_does_not_mark_ledger() {
        let reg: Vec<Box<dyn Migration>> = vec![Box::new(Stub {
            id: "safe",
            disruptive: false,
            state: MigrationState::Applicable,
        })];
        let ledger = FakeLedger::default();
        let mut rep = SetupReport::default();
        run_auto(&FakeSys::default(), &ledger, &reg, true, &mut rep).await;
        assert!(rep.repaired.iter().any(|r| r.contains("migration safe")));
        assert!(
            !ledger.is_applied("safe").await,
            "dry run must not record to the ledger"
        );
    }

    #[tokio::test]
    async fn dry_run_explicit_does_not_mark_ledger() {
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
            true,
            &mut rep,
        )
        .await;
        assert!(rep
            .repaired
            .iter()
            .any(|r| r.contains("migration nginx-to-caddy")));
        assert!(
            !ledger.is_applied("nginx-to-caddy").await,
            "dry run must not record to the ledger"
        );
    }

    #[tokio::test]
    async fn auto_noop_on_done() {
        let reg: Vec<Box<dyn Migration>> = vec![Box::new(Stub {
            id: "done-one",
            disruptive: false,
            state: MigrationState::Done,
        })];
        let ledger = FakeLedger::default();
        let mut rep = SetupReport::default();
        run_auto(&FakeSys::default(), &ledger, &reg, false, &mut rep).await;
        assert!(rep.repaired.is_empty());
        assert!(rep.warnings.is_empty());
        assert!(!ledger.is_applied("done-one").await);
    }

    #[tokio::test]
    async fn auto_noop_on_not_applicable() {
        let reg: Vec<Box<dyn Migration>> = vec![Box::new(Stub {
            id: "na-one",
            disruptive: false,
            state: MigrationState::NotApplicable,
        })];
        let ledger = FakeLedger::default();
        let mut rep = SetupReport::default();
        run_auto(&FakeSys::default(), &ledger, &reg, false, &mut rep).await;
        assert!(rep.repaired.is_empty());
        assert!(rep.warnings.is_empty());
        assert!(!ledger.is_applied("na-one").await);
    }

    #[tokio::test]
    async fn explicit_skips_done_with_message() {
        let reg: Vec<Box<dyn Migration>> = vec![Box::new(Stub {
            id: "done-one",
            disruptive: false,
            state: MigrationState::Done,
        })];
        let ledger = FakeLedger::default();
        let mut rep = SetupReport::default();
        run_explicit(
            &FakeSys::default(),
            &ledger,
            &reg,
            &["done-one".to_string()],
            false,
            &mut rep,
        )
        .await;
        assert!(rep.skipped.iter().any(|s| s.contains("already done")));
    }

    #[tokio::test]
    async fn explicit_skips_not_applicable_with_message() {
        let reg: Vec<Box<dyn Migration>> = vec![Box::new(Stub {
            id: "na-one",
            disruptive: false,
            state: MigrationState::NotApplicable,
        })];
        let ledger = FakeLedger::default();
        let mut rep = SetupReport::default();
        run_explicit(
            &FakeSys::default(),
            &ledger,
            &reg,
            &["na-one".to_string()],
            false,
            &mut rep,
        )
        .await;
        assert!(rep.skipped.iter().any(|s| s.contains("not applicable")));
    }

    #[tokio::test]
    async fn auto_skips_already_ledgered_without_detect() {
        let reg: Vec<Box<dyn Migration>> = vec![Box::new(Stub {
            id: "safe",
            disruptive: false,
            state: MigrationState::Applicable,
        })];
        let ledger = FakeLedger::default();
        ledger.mark_applied("safe").await;
        let mut rep = SetupReport::default();
        run_auto(&FakeSys::default(), &ledger, &reg, false, &mut rep).await;
        assert!(rep.repaired.is_empty());
        assert!(rep.warnings.is_empty());
        assert!(rep.errors.is_empty());
    }
}

#[cfg(test)]
mod pi_to_rpi_tests {
    use super::super::setup::fake::FakeSys;
    use super::*;

    fn legacy() -> FakeSys {
        let mut sys = FakeSys::default();
        sys.err.insert(FakeSys::key("id", &["-u", "rpi-agent"]));
        sys.ok
            .insert(FakeSys::key("id", &["-u", "pi-agent"]), "999".into());
        sys
    }

    #[tokio::test]
    async fn detects_legacy_install() {
        assert_eq!(PiToRpi.detect(&legacy()).await, MigrationState::Applicable);
    }

    #[tokio::test]
    async fn fresh_install_not_applicable() {
        let mut sys = FakeSys::default();
        sys.err.insert(FakeSys::key("id", &["-u", "rpi-agent"]));
        sys.err.insert(FakeSys::key("id", &["-u", "pi-agent"]));
        assert_eq!(PiToRpi.detect(&sys).await, MigrationState::NotApplicable);
    }

    #[tokio::test]
    async fn already_migrated_not_applicable() {
        // rpi-agent already exists -> guard must report NotApplicable, even
        // though pi-agent might still linger (already-migrated / fresh install).
        let mut sys = FakeSys::default();
        sys.ok
            .insert(FakeSys::key("id", &["-u", "rpi-agent"]), "999".into());
        assert_eq!(PiToRpi.detect(&sys).await, MigrationState::NotApplicable);
    }
}
