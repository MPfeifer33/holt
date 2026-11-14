//! Runs each agent's memory nightly when it is overdue.
//!
//! Until this existed the nightly ran inside the app only when an agent happened to call
//! `memory_cleanup`, and `[memory] nightly_interval_hours` was read by nothing.

use std::sync::Arc;
use std::time::Duration;

use crate::AppState;

/// How often the scheduler asks whether anyone is due.
const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Wait before the first check, so maintenance does not compete with app startup. Short,
/// because an app that is restarted more often than hourly must still get its nightly.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(5 * 60);

/// What to do about one agent's nightly at one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NightlyDecision {
    Run,
    /// `nightly_interval_hours = 0`.
    Disabled,
    NotDue,
    /// Due, but the agent is mid-turn; the next check will ask again.
    TurnActive,
}

/// Is this agent due? `overdue` is the engine's answer for `interval_hours`.
pub(crate) fn decide(interval_hours: u32, overdue: bool, turn_active: bool) -> NightlyDecision {
    if interval_hours == 0 {
        NightlyDecision::Disabled
    } else if !overdue {
        NightlyDecision::NotDue
    } else if turn_active {
        NightlyDecision::TurnActive
    } else {
        NightlyDecision::Run
    }
}

/// Start the scheduler. It stops with the app's shutdown token.
pub fn spawn(state: Arc<AppState>) {
    let shutdown = state.shutdown_token.clone();
    tauri::async_runtime::spawn(async move {
        let mut wait = FIRST_CHECK_DELAY;
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    tracing::info!("Memory nightly scheduler shutting down");
                    break;
                }
                _ = tokio::time::sleep(wait) => {
                    run_due_agents(&state).await;
                    wait = CHECK_INTERVAL;
                }
            }
        }
    });
}

/// One check: every agent with a memory space, one at a time. A failure is logged with
/// the agent and the rest still get their turn.
async fn run_due_agents(state: &AppState) {
    // Read each check, so a config change applies without a restart.
    let interval_hours = state.config.read().await.memory.nightly_interval_hours;
    if decide(interval_hours, true, false) == NightlyDecision::Disabled {
        return;
    }
    // Not there yet (or disabled): nothing to maintain. The next check asks again.
    let Some(engine) = state.get_memory_engine() else {
        return;
    };
    let memory_agents = match engine.agents_with_memory().await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!("Memory nightly scheduler: could not list memory spaces: {e}");
            return;
        }
    };
    let app_agent_ids: Vec<String> = state
        .agents
        .read()
        .await
        .iter()
        .map(|a| a.id.clone())
        .collect();

    for memory_agent in memory_agents {
        let overdue = match engine
            .is_nightly_overdue(&memory_agent, interval_hours)
            .await
        {
            Ok(overdue) => overdue,
            Err(e) => {
                tracing::warn!(
                    agent_id = %memory_agent,
                    "Memory nightly scheduler: could not read last run: {e}"
                );
                continue;
            }
        };
        // Several app agents can share one memory space (aliases), so any of them
        // being mid-turn holds the run back.
        let mut turn_active = false;
        for app_agent in app_agent_ids
            .iter()
            .filter(|id| engine.memory_agent_id_for(id) == memory_agent)
        {
            if state.turn_manager.is_active(app_agent).await {
                turn_active = true;
                break;
            }
        }

        match decide(interval_hours, overdue, turn_active) {
            NightlyDecision::Run => match engine.run_nightly(Some(&memory_agent), false).await {
                Ok(_) => tracing::info!(agent_id = %memory_agent, "Memory nightly complete"),
                Err(e) => {
                    tracing::warn!(agent_id = %memory_agent, "Memory nightly failed: {e}")
                }
            },
            NightlyDecision::TurnActive => {
                tracing::debug!(
                    agent_id = %memory_agent,
                    "Memory nightly due but the agent is mid-turn; will ask again"
                )
            }
            NightlyDecision::NotDue | NightlyDecision::Disabled => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_overdue_idle_agent_runs() {
        assert_eq!(decide(24, true, false), NightlyDecision::Run);
    }

    #[test]
    fn an_agent_that_ran_recently_waits() {
        assert_eq!(decide(24, false, false), NightlyDecision::NotDue);
        assert_eq!(decide(24, false, true), NightlyDecision::NotDue);
    }

    #[test]
    fn a_zero_interval_disables_the_schedule_whatever_else_is_true() {
        assert_eq!(decide(0, true, false), NightlyDecision::Disabled);
        assert_eq!(decide(0, false, true), NightlyDecision::Disabled);
    }

    #[test]
    fn maintenance_does_not_run_under_an_active_turn() {
        assert_eq!(decide(24, true, true), NightlyDecision::TurnActive);
    }

    /// The timings the module's contract states: a first check soon after startup, then
    /// hourly.
    #[test]
    fn the_first_check_is_five_minutes_in_and_the_rest_are_hourly() {
        assert_eq!(FIRST_CHECK_DELAY, Duration::from_secs(300));
        assert_eq!(CHECK_INTERVAL, Duration::from_secs(3600));
    }
}
