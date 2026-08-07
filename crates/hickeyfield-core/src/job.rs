//! Job status model.
//!
//! The status vocabulary mirrors Higgsfield's so the studio surfaces can show
//! the same stage labels. The intermediate variants are not noise: `Dna`,
//! `Script`, `Visuals`, `Vision` and `Flow` drive the multi-step progress copy
//! in Ad Studio and Explainer ("Extracting product DNA", "Writing a script").

use serde::{Deserialize, Serialize};

/// A raw per-job status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Pending,
    Waiting,
    Queued,
    Dna,
    Script,
    Visuals,
    Vision,
    Flow,
    InProgress,
    IpDetect,
    Completed,
    Failed,
    /// Refused by provider moderation. Terminal, and distinct from `Failed`
    /// because providers refund it and the user needs to know why.
    Nsfw,
    /// Refused for suspected third-party IP in the inputs.
    IpDetected,
    Canceled,
}

/// The coarse phase a job is in, for UI that only cares about
/// queued / running / done.
///
/// Two of these are **local** stages that no provider ever reports:
/// [`Phase::Uploading`] happens before the request exists and
/// [`Phase::Downloading`] after it is complete, so only this process can see
/// either. [`JobStatus::phase`] therefore cannot return them — use
/// [`Phase::observed`], which folds in what the runner is doing.
///
/// This is a label and only a label. There is no percentage and no ETA here,
/// deliberately: `ui/src/lib/status.ts` carries the policy note and it is
/// right. Provider queues give no honest basis for a number, and a bar that
/// reaches 100% before the file is on disk is a lie the user can catch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Queued,
    /// The user's input media is being pushed to provider storage. Nothing has
    /// been submitted yet, so there is no provider status to report: the row
    /// exists and reads "In queue" while the bytes have not left the machine,
    /// which for an attachment near fal's 90 MB single-shot ceiling is the
    /// longest wholly unexplained wait in the product.
    Uploading,
    InProgress,
    /// The provider says it is done and we are pulling the bytes onto disk.
    ///
    /// Distinct from [`Phase::Completed`] because a result that exists only on
    /// the provider's CDN is not finished — those URLs expire, and Higgsfield's
    /// own API deletes results after 7 days. Labelling this "Ready" invites the
    /// user to quit during the one window in which the output can still be
    /// lost, and it is the same window `JobSet::is_settled` already refuses to
    /// call settled.
    Downloading,
    Completed,
    Failed,
    Canceled,
}

/// What this process is doing for a job, independent of what the provider says.
///
/// Kept separate from [`JobStatus`] rather than added to it because these are
/// not provider statuses. `JobStatus` is the vocabulary
/// `clients::normalize_status` parses off the wire; a `JobStatus::Uploading`
/// would be a variant of that vocabulary that no provider string can ever map
/// to, and the next person to read the enum would go looking for the response
/// field that produces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LocalStage {
    /// Nothing local is in flight; the provider's status is the whole story.
    #[default]
    Idle,
    Uploading,
    Downloading,
}

impl LocalStage {
    /// The stage a polled job is in, derived from what the runner already
    /// knows. `outputs_pending` is "the provider handed us outputs we have not
    /// saved yet".
    ///
    /// Exists so the poll loop and the DTO layer cannot disagree about when a
    /// job is downloading — they would otherwise each re-derive it, and the one
    /// that got it wrong would show "Ready" over an expiring link.
    pub fn after_poll(status: JobStatus, outputs_pending: bool) -> LocalStage {
        if status.phase() == Phase::Completed && outputs_pending {
            LocalStage::Downloading
        } else {
            LocalStage::Idle
        }
    }
}

impl Phase {
    /// The phase to show, combining the provider's status with the local stage
    /// this process is in.
    ///
    /// A terminal failure or cancellation outranks any local stage: a job
    /// cancelled while its inputs were still uploading is cancelled, not
    /// uploading. Letting the stage win would leave a card stuck mid-progress
    /// forever, since nothing will ever clear a local stage on a job that has
    /// already stopped.
    ///
    /// [`Phase::Completed`] deliberately does *not* outrank
    /// [`LocalStage::Downloading`]. That pair is the reason this function
    /// exists: the provider is finished and we are not.
    pub fn observed(status: JobStatus, local: LocalStage) -> Phase {
        let reported = status.phase();
        match local {
            LocalStage::Idle => reported,
            _ if matches!(reported, Phase::Failed | Phase::Canceled) => reported,
            LocalStage::Uploading => Phase::Uploading,
            LocalStage::Downloading => Phase::Downloading,
        }
    }
}

impl JobStatus {
    pub fn phase(self) -> Phase {
        use JobStatus::*;
        match self {
            Pending | Waiting | Queued => Phase::Queued,
            Dna | Script | Visuals | Vision | Flow | InProgress | IpDetect => Phase::InProgress,
            Completed => Phase::Completed,
            Failed | Nsfw | IpDetected => Phase::Failed,
            Canceled => Phase::Canceled,
        }
    }

    /// Terminal statuses stop the poller.
    pub fn is_terminal(self) -> bool {
        matches!(
            self.phase(),
            Phase::Completed | Phase::Failed | Phase::Canceled
        )
    }

    /// Whether the provider refunds this outcome. Both moderation refusals are
    /// refunded by every provider we route to; a generic failure usually is too.
    pub fn is_refunded(self) -> bool {
        matches!(self, JobStatus::Failed | JobStatus::Nsfw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intermediate_stages_are_in_progress() {
        for s in [
            JobStatus::Dna,
            JobStatus::Script,
            JobStatus::Visuals,
            JobStatus::Vision,
            JobStatus::Flow,
            JobStatus::IpDetect,
        ] {
            assert_eq!(s.phase(), Phase::InProgress, "{s:?} should be in_progress");
            assert!(!s.is_terminal(), "{s:?} must not stop the poller");
        }
    }

    #[test]
    fn moderation_refusals_are_terminal_failures_not_successes() {
        for s in [JobStatus::Nsfw, JobStatus::IpDetected] {
            assert_eq!(s.phase(), Phase::Failed);
            assert!(s.is_terminal());
        }
    }

    #[test]
    fn cancel_is_its_own_phase() {
        // Canceled is terminal but must not render as an error to the user.
        assert_eq!(JobStatus::Canceled.phase(), Phase::Canceled);
        assert!(JobStatus::Canceled.is_terminal());
    }

    #[test]
    fn queued_states_are_not_terminal() {
        for s in [JobStatus::Pending, JobStatus::Waiting, JobStatus::Queued] {
            assert_eq!(s.phase(), Phase::Queued);
            assert!(!s.is_terminal());
        }
    }

    /// Every `JobStatus`, so the tests below cannot quietly miss a new one.
    const ALL_STATUSES: [JobStatus; 15] = [
        JobStatus::Pending,
        JobStatus::Waiting,
        JobStatus::Queued,
        JobStatus::Dna,
        JobStatus::Script,
        JobStatus::Visuals,
        JobStatus::Vision,
        JobStatus::Flow,
        JobStatus::InProgress,
        JobStatus::IpDetect,
        JobStatus::Completed,
        JobStatus::Failed,
        JobStatus::Nsfw,
        JobStatus::IpDetected,
        JobStatus::Canceled,
    ];

    #[test]
    fn no_provider_status_can_produce_a_local_stage() {
        // Uploading and Downloading happen on this machine, before the request
        // exists and after it is complete. If `phase()` ever returned one, some
        // provider string would be being mapped to a stage that provider cannot
        // observe.
        for s in ALL_STATUSES {
            assert!(
                !matches!(s.phase(), Phase::Uploading | Phase::Downloading),
                "{s:?} collapsed to a local-only phase"
            );
        }
    }

    #[test]
    fn a_completed_job_whose_bytes_are_still_moving_is_not_ready() {
        // The lie this prevents: the feed says "Ready" the instant the provider
        // says completed, while the card still links the provider's signed,
        // expiring URL and the file is not on disk yet.
        assert_eq!(
            Phase::observed(JobStatus::Completed, LocalStage::Downloading),
            Phase::Downloading
        );
        assert_eq!(
            Phase::observed(JobStatus::Completed, LocalStage::Idle),
            Phase::Completed
        );
    }

    #[test]
    fn a_terminal_failure_outranks_a_local_stage() {
        // Otherwise a job cancelled mid-upload renders as still uploading — a
        // card that never resolves and a Cancel button that appears to do
        // nothing.
        for (status, stage) in [
            (JobStatus::Canceled, LocalStage::Uploading),
            (JobStatus::Failed, LocalStage::Uploading),
            (JobStatus::Nsfw, LocalStage::Downloading),
        ] {
            assert_eq!(Phase::observed(status, stage), status.phase());
        }
    }

    #[test]
    fn uploading_covers_the_gap_before_a_provider_has_anything_to_say() {
        for s in [JobStatus::Pending, JobStatus::Queued, JobStatus::Waiting] {
            assert_eq!(Phase::observed(s, LocalStage::Uploading), Phase::Uploading);
        }
    }

    #[test]
    fn downloading_is_only_claimed_when_there_is_something_left_to_fetch() {
        assert_eq!(
            LocalStage::after_poll(JobStatus::Completed, true),
            LocalStage::Downloading
        );
        assert_eq!(
            LocalStage::after_poll(JobStatus::Completed, false),
            LocalStage::Idle
        );
        // A failed job has nothing to download even if outputs were reported.
        assert_eq!(
            LocalStage::after_poll(JobStatus::Failed, true),
            LocalStage::Idle
        );
        assert_eq!(
            LocalStage::after_poll(JobStatus::InProgress, true),
            LocalStage::Idle
        );
    }

    #[test]
    fn serde_uses_the_wire_names() {
        // These strings come off provider responses; renaming them silently
        // would break status parsing.
        assert_eq!(
            serde_json::to_string(&JobStatus::InProgress).unwrap(),
            "\"in_progress\""
        );
        assert_eq!(
            serde_json::to_string(&JobStatus::IpDetected).unwrap(),
            "\"ip_detected\""
        );
        let parsed: JobStatus = serde_json::from_str("\"nsfw\"").unwrap();
        assert_eq!(parsed, JobStatus::Nsfw);
    }

    #[test]
    fn the_local_phases_serialise_as_snake_case_labels() {
        // These strings cross the bridge into `ui/src/lib/status.ts`, which
        // keys its copy off them.
        assert_eq!(
            serde_json::to_string(&Phase::Uploading).unwrap(),
            "\"uploading\""
        );
        assert_eq!(
            serde_json::to_string(&Phase::Downloading).unwrap(),
            "\"downloading\""
        );
    }
}
