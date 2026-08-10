use crate::annotation::AnnotationKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommentOrigin {
    Human,
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommentLifecycle {
    Open,
    Moved,
    Stale,
    Cleared,
}

impl CommentLifecycle {
    pub(crate) fn is_visible(self) -> bool {
        matches!(self, Self::Open | Self::Moved)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FindingDisposition {
    Open,
    Accepted,
    Dismissed,
    Blocking,
    NonBlocking,
    Fixed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ReviewAnchorEvidence {
    pub(crate) file_fingerprint: Option<String>,
    pub(crate) old_lines: Vec<String>,
    pub(crate) new_lines: Vec<String>,
    pub(crate) hunk_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReviewComment {
    pub(crate) id: String,
    pub(crate) anchor: AnnotationKey,
    pub(crate) summary: String,
    pub(crate) rationale: Option<String>,
    pub(crate) author: Option<String>,
    pub(crate) origin: CommentOrigin,
    pub(crate) lifecycle: CommentLifecycle,
    pub(crate) disposition: FindingDisposition,
    pub(crate) document_generation: u64,
    pub(crate) original_anchor_evidence: Option<ReviewAnchorEvidence>,
}

#[derive(Debug, Clone)]
pub(crate) struct NewAgentComment {
    pub(crate) anchor: AnnotationKey,
    pub(crate) summary: String,
    pub(crate) rationale: Option<String>,
    pub(crate) author: Option<String>,
}
