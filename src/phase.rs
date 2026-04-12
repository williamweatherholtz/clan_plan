use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};

/// All phases a reunion moves through, in order.
/// The RA (responsible admin) opens/closes each transition.
/// Some actions (e.g. posting activity ideas) are allowed in any phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "reunion_phase", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// RA is setting up the reunion record; not yet visible to members.
    Draft,
    /// Members mark which days work for them.
    Availability,
    /// RA has reviewed the heatmap and locked in the date range.
    DateSelected,
    /// RA has added location candidates; members vote (blind until revealed).
    Locations,
    /// RA has picked the location.
    LocationSelected,
    /// RA is building the daily schedule blocks.
    Schedule,
    /// The reunion is happening; full access — today-view, signups, media, expenses.
    Active,
    /// Reunion is over; post-event survey is open; read-mostly.
    PostReunion,
    /// Permanent read-only archive.
    Archived,
}

impl Phase {
    /// Returns the next phase in the sequence, or `None` if already archived.
    pub fn next(&self) -> Option<Phase> {
        match self {
            Phase::Draft => Some(Phase::Availability),
            Phase::Availability => Some(Phase::DateSelected),
            Phase::DateSelected => Some(Phase::Locations),
            Phase::Locations => Some(Phase::LocationSelected),
            Phase::LocationSelected => Some(Phase::Schedule),
            Phase::Schedule => Some(Phase::Active),
            Phase::Active => Some(Phase::PostReunion),
            Phase::PostReunion => Some(Phase::Archived),
            Phase::Archived => None,
        }
    }

    /// Advance to the next phase, or error if already archived.
    pub fn advance(&self) -> AppResult<Phase> {
        self.next()
            .ok_or_else(|| AppError::BadRequest("reunion is already archived".into()))
    }

    /// Whether a direct transition from `self` → `to` is valid.
    /// Only sequential forward steps are permitted; no skipping, no reverting.
    pub fn can_advance_to(&self, to: &Phase) -> bool {
        self.next().as_ref() == Some(to)
    }

    /// Human-readable label shown in the UI.
    pub fn label(&self) -> &'static str {
        match self {
            Phase::Draft => "Draft",
            Phase::Availability => "Collecting Availability",
            Phase::DateSelected => "Dates Confirmed",
            Phase::Locations => "Voting on Locations",
            Phase::LocationSelected => "Location Confirmed",
            Phase::Schedule => "Building Schedule",
            Phase::Active => "Reunion Active",
            Phase::PostReunion => "Post-Reunion",
            Phase::Archived => "Archived",
        }
    }
}

/// Guard helper: returns `Ok(())` if `current` is one of the `allowed` phases,
/// or a `WrongPhase` error describing the mismatch.
pub fn require_phase(current: &Phase, allowed: &[Phase]) -> AppResult<()> {
    if allowed.contains(current) {
        Ok(())
    } else {
        let required = allowed
            .iter()
            .map(|p| p.label())
            .collect::<Vec<_>>()
            .join(" or ");
        Err(AppError::WrongPhase {
            required,
            current: current.label().into(),
        })
    }
}

// ── Activity ideas are intentionally unrestricted by phase ──────────────────
// Any member may post/vote on ideas from Draft onward. This is enforced at the
// route level by checking only that the user is authenticated, not the phase.

#[cfg(test)]
mod tests {
    use super::*;

    fn all_phases() -> Vec<Phase> {
        vec![
            Phase::Draft,
            Phase::Availability,
            Phase::DateSelected,
            Phase::Locations,
            Phase::LocationSelected,
            Phase::Schedule,
            Phase::Active,
            Phase::PostReunion,
            Phase::Archived,
        ]
    }

    #[test]
    fn phases_advance_sequentially() {
        let phases = all_phases();
        for window in phases.windows(2) {
            let current = &window[0];
            let next = &window[1];
            assert!(
                current.can_advance_to(next),
                "{:?} should be able to advance to {:?}",
                current,
                next
            );
            assert_eq!(current.next().as_ref(), Some(next));
        }
    }

    #[test]
    fn archived_has_no_successor() {
        assert_eq!(Phase::Archived.next(), None);
        assert!(Phase::Archived.advance().is_err());
    }

    #[test]
    fn cannot_skip_phases() {
        assert!(!Phase::Draft.can_advance_to(&Phase::Active));
        assert!(!Phase::Draft.can_advance_to(&Phase::Locations));
        assert!(!Phase::Availability.can_advance_to(&Phase::Schedule));
        assert!(!Phase::DateSelected.can_advance_to(&Phase::Active));
    }

    #[test]
    fn cannot_revert_phases() {
        assert!(!Phase::Active.can_advance_to(&Phase::Draft));
        assert!(!Phase::Schedule.can_advance_to(&Phase::Availability));
        assert!(!Phase::PostReunion.can_advance_to(&Phase::Active));
    }

    #[test]
    fn require_phase_passes_when_allowed() {
        let result = require_phase(&Phase::Availability, &[Phase::Availability, Phase::Draft]);
        assert!(result.is_ok());
    }

    #[test]
    fn require_phase_fails_when_wrong() {
        let result = require_phase(&Phase::Active, &[Phase::Draft]);
        assert!(matches!(result, Err(AppError::WrongPhase { .. })));
    }

    #[test]
    fn every_phase_has_a_label() {
        for phase in all_phases() {
            assert!(!phase.label().is_empty());
        }
    }
}
