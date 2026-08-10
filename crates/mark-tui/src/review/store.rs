use std::collections::{HashMap, HashSet};

use crate::annotation::AnnotationKey;

use super::comment::{
    CommentLifecycle, CommentOrigin, FindingDisposition, NewAgentComment, ReviewComment,
};

#[derive(Debug, Default)]
pub(crate) struct ReviewCommentStore {
    next_id: u64,
    comments: Vec<Option<ReviewComment>>,
    by_id: HashMap<String, usize>,
    anchors: HashMap<AnnotationKey, AnchorView>,
    comment_count: usize,
}

#[derive(Debug, Default)]
pub(crate) struct AnchorView {
    ids: Vec<String>,
    text: String,
    label: String,
}

impl ReviewCommentStore {
    pub(crate) fn len(&self) -> usize {
        self.comment_count
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.comment_count == 0
    }

    #[cfg(test)]
    pub(crate) fn anchor_count(&self) -> usize {
        self.anchors.len()
    }

    pub(crate) fn keys(&self) -> impl Iterator<Item = &AnnotationKey> {
        self.anchors.keys()
    }

    pub(crate) fn get(&self, key: &AnnotationKey) -> Option<&String> {
        self.anchors.get(key).map(|view| &view.text)
    }

    pub(crate) fn contains_key(&self, key: &AnnotationKey) -> bool {
        self.anchors.contains_key(key)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&AnnotationKey, &String)> {
        self.anchors.iter().map(|(key, view)| (key, &view.text))
    }

    pub(crate) fn comments(&self) -> impl Iterator<Item = &ReviewComment> {
        self.comments.iter().filter_map(Option::as_ref)
    }

    pub(crate) fn label(&self, key: &AnnotationKey) -> Option<&str> {
        self.anchors.get(key).map(|view| view.label.as_str())
    }

    pub(crate) fn human_text(&self, key: &AnnotationKey) -> Option<&str> {
        self.anchors.get(key)?.ids.iter().find_map(|id| {
            let index = *self.by_id.get(id)?;
            let comment = self.comments.get(index)?.as_ref()?;
            (comment.origin == CommentOrigin::Human).then_some(comment.summary.as_str())
        })
    }

    pub(crate) fn has_human(&self, key: &AnnotationKey) -> bool {
        self.comments_at(key)
            .any(|comment| comment.origin == CommentOrigin::Human)
    }

    pub(crate) fn has_agent(&self, key: &AnnotationKey) -> bool {
        self.comments_at(key)
            .any(|comment| comment.origin == CommentOrigin::Agent)
    }

    pub(crate) fn is_human_only(&self, key: &AnnotationKey) -> bool {
        self.has_human(key) && !self.has_agent(key)
    }

    #[cfg(test)]
    pub(crate) fn insert(&mut self, anchor: AnnotationKey, text: String) -> Option<String> {
        self.insert_human(anchor, text, 0)
            .expect("test annotation should fit store limits")
    }

    #[cfg(test)]
    pub(crate) fn insert_human(
        &mut self,
        anchor: AnnotationKey,
        text: String,
        generation: u64,
    ) -> Result<Option<String>, StoreLimitError> {
        self.insert_human_inner(anchor, text, generation)
    }

    pub(crate) fn insert_human_with_budget(
        &mut self,
        anchor: AnnotationKey,
        text: String,
        generation: u64,
        _budget: HumanCommentPersistenceBudget,
    ) -> Result<Option<String>, StoreLimitError> {
        self.insert_human_inner(anchor, text, generation)
    }

    fn insert_human_inner(
        &mut self,
        anchor: AnnotationKey,
        text: String,
        generation: u64,
    ) -> Result<Option<String>, StoreLimitError> {
        if anchor.path.is_empty()
            || anchor.path.len() > mark_session::MAX_PATH_BYTES
            || text.len() > mark_session::MAX_RATIONALE_BYTES
        {
            return Err(StoreLimitError);
        }
        let anchor = self.canonical_anchor(&anchor);
        if let Some(index) = self.human_index(&anchor) {
            let comment = self.comments[index]
                .as_mut()
                .expect("indexed comment should exist");
            let previous = std::mem::replace(&mut comment.summary, text);
            comment.document_generation = generation;
            self.rebuild_anchor(&anchor);
            return Ok(Some(previous));
        }
        if self.comment_count >= mark_session::MAX_LIVE_COMMENTS {
            return Err(StoreLimitError);
        }
        let id = self.next_comment_id("human");
        self.insert_comment(ReviewComment {
            id,
            anchor,
            summary: text,
            rationale: None,
            author: None,
            origin: CommentOrigin::Human,
            lifecycle: CommentLifecycle::Open,
            disposition: FindingDisposition::Open,
            document_generation: generation,
            evidence: None,
        });
        Ok(None)
    }

    pub(crate) fn restore_comments(
        &mut self,
        comments: Vec<ReviewComment>,
    ) -> Result<(), StoreLimitError> {
        if comments.len() > mark_session::MAX_LIVE_COMMENTS {
            return Err(StoreLimitError);
        }
        let mut ids = HashSet::with_capacity(comments.len());
        let mut next_id = 0u64;
        for comment in &comments {
            let expected_prefix = match comment.origin {
                CommentOrigin::Human => "human-",
                CommentOrigin::Agent => "agent-",
            };
            let Some(sequence) = comment
                .id
                .strip_prefix(expected_prefix)
                .and_then(|value| value.parse::<u64>().ok())
            else {
                return Err(StoreLimitError);
            };
            let summary_limit = match comment.origin {
                CommentOrigin::Human => mark_session::MAX_RATIONALE_BYTES,
                CommentOrigin::Agent => mark_session::MAX_SUMMARY_BYTES,
            };
            if !ids.insert(comment.id.clone())
                || comment.anchor.path.is_empty()
                || comment.anchor.path.len() > mark_session::MAX_PATH_BYTES
                || comment.summary.len() > summary_limit
                || comment
                    .rationale
                    .as_ref()
                    .is_some_and(|text| text.len() > mark_session::MAX_RATIONALE_BYTES)
                || comment
                    .author
                    .as_ref()
                    .is_some_and(|text| text.len() > mark_session::MAX_AUTHOR_BYTES)
            {
                return Err(StoreLimitError);
            }
            next_id = next_id.max(sequence);
        }

        *self = Self::default();
        self.next_id = next_id;
        for comment in comments {
            self.insert_comment(comment);
        }
        Ok(())
    }

    pub(crate) fn insert_agent_batch(
        &mut self,
        comments: Vec<NewAgentComment>,
        generation: u64,
    ) -> Result<Vec<String>, StoreLimitError> {
        if comments.len() > mark_session::MAX_COMMENTS_PER_BATCH
            || self.comment_count.saturating_add(comments.len()) > mark_session::MAX_LIVE_COMMENTS
            || comments.iter().any(|comment| {
                comment.anchor.path.is_empty()
                    || comment.anchor.path.len() > mark_session::MAX_PATH_BYTES
                    || comment.summary.len() > mark_session::MAX_SUMMARY_BYTES
                    || comment
                        .rationale
                        .as_ref()
                        .is_some_and(|text| text.len() > mark_session::MAX_RATIONALE_BYTES)
                    || comment
                        .author
                        .as_ref()
                        .is_some_and(|text| text.len() > mark_session::MAX_AUTHOR_BYTES)
            })
        {
            return Err(StoreLimitError);
        }
        let mut ids = Vec::with_capacity(comments.len());
        for comment in comments {
            let id = self.next_comment_id("agent");
            ids.push(id.clone());
            let anchor = self.canonical_anchor(&comment.anchor);
            self.insert_comment(ReviewComment {
                id,
                anchor,
                summary: comment.summary,
                rationale: comment.rationale,
                author: comment.author,
                origin: CommentOrigin::Agent,
                lifecycle: CommentLifecycle::Open,
                disposition: FindingDisposition::Open,
                document_generation: generation,
                evidence: None,
            });
        }
        Ok(ids)
    }

    pub(crate) fn remove(&mut self, anchor: &AnnotationKey) -> Option<String> {
        let previous = self.get(anchor).cloned()?;
        if self.has_agent(anchor) {
            self.remove_agents_at(anchor);
        } else {
            self.remove_human(anchor)?;
        }
        Some(previous)
    }

    pub(crate) fn remove_human(&mut self, anchor: &AnnotationKey) -> Option<String> {
        let index = self.human_index(anchor)?;
        let id = self.comments[index].as_ref()?.id.clone();
        let summary = self.comments[index].as_ref()?.summary.clone();
        self.remove_any_by_id(&id);
        Some(summary)
    }

    pub(crate) fn set_disposition(
        &mut self,
        id: &str,
        disposition: FindingDisposition,
    ) -> Result<(), ()> {
        let Some(index) = self.by_id.get(id).copied() else {
            return Err(());
        };
        let Some(comment) = self.comments[index].as_mut() else {
            return Err(());
        };
        if comment.origin != CommentOrigin::Agent {
            return Err(());
        }
        comment.disposition = disposition;
        self.rebuild_all_anchors();
        Ok(())
    }

    pub(crate) fn set_agents_disposition_at(
        &mut self,
        anchor: &AnnotationKey,
        disposition: FindingDisposition,
    ) -> usize {
        let ids = self
            .comments_at(anchor)
            .filter(|comment| comment.origin == CommentOrigin::Agent)
            .map(|comment| comment.id.clone())
            .collect::<Vec<_>>();
        for id in &ids {
            if let Some(comment) = self
                .by_id
                .get(id)
                .and_then(|index| self.comments[*index].as_mut())
            {
                comment.disposition = disposition;
            }
        }
        if !ids.is_empty() {
            self.rebuild_all_anchors();
        }
        ids.len()
    }

    pub(crate) fn remove_agent_by_id(&mut self, id: &str) -> Result<bool, ()> {
        let Some(index) = self.by_id.get(id).copied() else {
            return Ok(false);
        };
        if self.comments[index]
            .as_ref()
            .is_some_and(|comment| comment.origin == CommentOrigin::Human)
        {
            return Err(());
        }
        self.remove_any_by_id(id);
        Ok(true)
    }

    pub(crate) fn clear_agents(&mut self, file: Option<&str>) -> usize {
        let ids = self
            .comments()
            .filter(|comment| {
                comment.origin == CommentOrigin::Agent
                    && file.is_none_or(|file| comment.anchor.path == file)
            })
            .map(|comment| comment.id.clone())
            .collect::<Vec<_>>();
        self.remove_agent_ids(ids)
    }

    pub(crate) fn remove_agents_at(&mut self, anchor: &AnnotationKey) -> usize {
        let ids = self
            .comments_at(anchor)
            .filter(|comment| comment.origin == CommentOrigin::Agent)
            .map(|comment| comment.id.clone())
            .collect::<Vec<_>>();
        self.remove_agent_ids(ids)
    }

    fn insert_comment(&mut self, comment: ReviewComment) {
        let anchor = comment.anchor.clone();
        let id = comment.id.clone();
        let index = self.comments.len();
        let visible =
            comment.lifecycle.is_visible() && comment.disposition != FindingDisposition::Dismissed;
        self.comments.push(Some(comment));
        self.by_id.insert(id.clone(), index);
        if visible {
            self.anchors.entry(anchor.clone()).or_default().ids.push(id);
            self.rebuild_anchor(&anchor);
        }
        self.comment_count = self.comment_count.saturating_add(1);
    }

    fn remove_any_by_id(&mut self, id: &str) {
        let Some(index) = self.by_id.remove(id) else {
            return;
        };
        let Some(comment) = self.comments.get_mut(index).and_then(Option::take) else {
            return;
        };
        self.comment_count = self.comment_count.saturating_sub(1);
        if let Some(view) = self.anchors.get_mut(&comment.anchor) {
            view.ids.retain(|candidate| candidate != id);
            if view.ids.is_empty() {
                self.anchors.remove(&comment.anchor);
                return;
            }
        }
        self.rebuild_anchor(&comment.anchor);
    }

    fn human_index(&self, anchor: &AnnotationKey) -> Option<usize> {
        self.anchors.get(anchor)?.ids.iter().find_map(|id| {
            let index = self.by_id.get(id).copied()?;
            self.comments[index]
                .as_ref()
                .filter(|comment| comment.origin == CommentOrigin::Human)
                .map(|_| index)
        })
    }

    fn comments_at<'a>(
        &'a self,
        anchor: &'a AnnotationKey,
    ) -> impl Iterator<Item = &'a ReviewComment> + 'a {
        self.anchors
            .get(anchor)
            .into_iter()
            .flat_map(|view| view.ids.iter())
            .filter_map(|id| self.by_id.get(id))
            .filter_map(|index| self.comments[*index].as_ref())
    }

    pub(crate) fn canonical_anchor(&self, anchor: &AnnotationKey) -> AnnotationKey {
        self.anchors
            .keys()
            .find(|candidate| same_rendered_anchor(candidate, anchor))
            .cloned()
            .unwrap_or_else(|| anchor.clone())
    }

    fn remove_agent_ids(&mut self, ids: Vec<String>) -> usize {
        let removed = ids.len();
        for id in ids {
            self.remove_any_by_id(&id);
        }
        removed
    }

    fn next_comment_id(&mut self, prefix: &str) -> String {
        self.next_id = self.next_id.saturating_add(1);
        format!("{prefix}-{}", self.next_id)
    }

    fn rebuild_all_anchors(&mut self) {
        self.anchors.clear();
        for comment in self.comments.iter().filter_map(Option::as_ref) {
            if comment.lifecycle.is_visible()
                && comment.disposition != FindingDisposition::Dismissed
            {
                self.anchors
                    .entry(comment.anchor.clone())
                    .or_default()
                    .ids
                    .push(comment.id.clone());
            }
        }
        let anchors = self.anchors.keys().cloned().collect::<Vec<_>>();
        for anchor in anchors {
            self.rebuild_anchor(&anchor);
        }
    }

    fn rebuild_anchor(&mut self, anchor: &AnnotationKey) {
        let Some(ids) = self.anchors.get(anchor).map(|view| view.ids.clone()) else {
            return;
        };
        let comments = ids
            .iter()
            .filter_map(|id| self.by_id.get(id))
            .filter_map(|index| self.comments[*index].as_ref())
            .collect::<Vec<_>>();
        let text = render_comments(&comments);
        let label = comment_label(&comments);
        if let Some(view) = self.anchors.get_mut(anchor) {
            view.text = text;
            view.label = label;
        }
    }
}

pub(crate) struct HumanCommentPersistenceBudget(());

impl HumanCommentPersistenceBudget {
    pub(super) fn verified() -> Self {
        Self(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StoreLimitError;

impl std::ops::Index<&AnnotationKey> for ReviewCommentStore {
    type Output = String;

    fn index(&self, key: &AnnotationKey) -> &Self::Output {
        &self.anchors[key].text
    }
}

impl<'a> IntoIterator for &'a ReviewCommentStore {
    type Item = (&'a AnnotationKey, &'a String);
    type IntoIter = std::iter::Map<
        std::collections::hash_map::Iter<'a, AnnotationKey, AnchorView>,
        fn((&'a AnnotationKey, &'a AnchorView)) -> (&'a AnnotationKey, &'a String),
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.anchors.iter().map(anchor_entry)
    }
}

fn anchor_entry<'a>(
    (key, view): (&'a AnnotationKey, &'a AnchorView),
) -> (&'a AnnotationKey, &'a String) {
    (key, &view.text)
}

fn same_rendered_anchor(left: &AnnotationKey, right: &AnnotationKey) -> bool {
    left == right
        || (left.path == right.path
            && left.scope == right.scope
            && !matches!(left.scope, crate::annotation::AnnotationScope::Line))
}

fn render_comments(comments: &[&ReviewComment]) -> String {
    if comments.len() == 1 {
        let comment = comments[0];
        if comment.origin == CommentOrigin::Human {
            return comment.summary.clone();
        }
        return match comment.rationale.as_deref() {
            Some(rationale) if !rationale.is_empty() => {
                format!("{}\n\n{rationale}", comment.summary)
            }
            _ => comment.summary.clone(),
        };
    }

    let mut rendered = String::new();
    for (index, comment) in comments.iter().enumerate() {
        if index > 0 {
            rendered.push_str("\n\n");
        }
        match comment.origin {
            CommentOrigin::Human => rendered.push_str("Human: "),
            CommentOrigin::Agent => {
                rendered.push_str("Agent");
                if let Some(author) = comment
                    .author
                    .as_deref()
                    .filter(|author| !author.is_empty())
                {
                    rendered.push_str(" (");
                    rendered.push_str(author);
                    rendered.push(')');
                }
                rendered.push_str(": ");
            }
        }
        rendered.push_str(&comment.summary);
        if let Some(rationale) = comment.rationale.as_deref().filter(|text| !text.is_empty()) {
            rendered.push('\n');
            rendered.push_str(rationale);
        }
    }
    rendered
}

fn comment_label(comments: &[&ReviewComment]) -> String {
    match comments {
        [comment] if comment.origin == CommentOrigin::Human => lifecycle_label("Human", comment),
        [comment] => {
            let origin = comment
                .author
                .as_deref()
                .map_or_else(|| "Agent".to_owned(), |author| format!("Agent · {author}"));
            lifecycle_label(&origin, comment)
        }
        comments => {
            let mut label = format!("{} comments", comments.len());
            if comments
                .iter()
                .any(|comment| comment.lifecycle == CommentLifecycle::Moved)
            {
                label.push_str(" · moved");
            }
            let mut dispositions = comments
                .iter()
                .filter(|comment| comment.origin == CommentOrigin::Agent)
                .map(|comment| comment.disposition);
            if let Some(disposition) = dispositions.next()
                && disposition != FindingDisposition::Open
                && dispositions.all(|candidate| candidate == disposition)
            {
                label.push_str(" · ");
                label.push_str(
                    disposition_name(disposition)
                        .expect("non-open finding dispositions should have a label"),
                );
            }
            label
        }
    }
}

fn lifecycle_label(prefix: &str, comment: &ReviewComment) -> String {
    let mut label = prefix.to_owned();
    if comment.lifecycle == CommentLifecycle::Moved {
        label.push_str(" · moved");
    }
    let disposition = disposition_name(comment.disposition);
    if let Some(disposition) = disposition {
        label.push_str(" · ");
        label.push_str(disposition);
    }
    label
}

fn disposition_name(disposition: FindingDisposition) -> Option<&'static str> {
    match disposition {
        FindingDisposition::Open => None,
        FindingDisposition::Accepted => Some("accepted"),
        FindingDisposition::Dismissed => Some("dismissed"),
        FindingDisposition::Blocking => Some("blocking"),
        FindingDisposition::NonBlocking => Some("non-blocking"),
        FindingDisposition::Fixed => Some("fixed"),
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        annotation::{AnnotationScope, AnnotationSide},
        render::{
            annotation_ranges::AnnotationBlockGeometry, annotations::render_annotation_saved_block,
        },
        theme::DiffTheme,
    };

    use super::*;

    fn anchor() -> AnnotationKey {
        AnnotationKey {
            path: "src/lib.rs".to_owned(),
            side: AnnotationSide::New,
            line: 4,
            scope: AnnotationScope::Line,
        }
    }

    fn range_anchor(side: AnnotationSide, line: usize) -> AnnotationKey {
        AnnotationKey {
            path: "src/lib.rs".to_owned(),
            side,
            line,
            scope: AnnotationScope::Range {
                old_start: 37,
                old_count: 5,
                new_start: 36,
                new_count: 10,
            },
        }
    }

    #[test]
    fn multiple_comments_share_an_anchor_with_stable_ids() {
        let mut store = ReviewCommentStore::default();
        store
            .insert_human(anchor(), "human note".to_owned(), 1)
            .unwrap();
        let ids = store
            .insert_agent_batch(
                vec![NewAgentComment {
                    anchor: anchor(),
                    summary: "agent finding".to_owned(),
                    rationale: Some("reason".to_owned()),
                    author: Some("codex".to_owned()),
                }],
                1,
            )
            .unwrap();

        assert_eq!(store.len(), 2);
        assert_eq!(store.anchor_count(), 1);
        assert_eq!(ids, ["agent-2"]);
        assert!(store.get(&anchor()).unwrap().contains("Human:"));
        assert!(store.get(&anchor()).unwrap().contains("Agent (codex):"));
    }

    #[test]
    fn saved_agent_cards_escape_all_external_text_fields() {
        let mut store = ReviewCommentStore::default();
        store
            .insert_agent_batch(
                vec![NewAgentComment {
                    anchor: anchor(),
                    summary: "summary\u{1b}]52;c;summary\u{7}".to_owned(),
                    rationale: Some("rationale\u{1b}]52;c;rationale\u{7}".to_owned()),
                    author: Some("author\u{1b}]52;c;author\u{7}".to_owned()),
                }],
                1,
            )
            .unwrap();

        let lines = render_annotation_saved_block(
            store.get(&anchor()).unwrap(),
            240,
            AnnotationBlockGeometry {
                start: 0,
                end: 240,
                connected: false,
            },
            DiffTheme::default(),
            store.label(&anchor()),
            false,
        );
        let rendered = lines
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!rendered.contains('\u{1b}'));
        assert!(!rendered.contains('\u{7}'));
        assert!(rendered.contains("summary\\u{1b}]52;c;summary\\u{7}"));
        assert!(rendered.contains("rationale\\u{1b}]52;c;rationale\\u{7}"));
        assert!(rendered.contains("author\\u{1b}]52;c;author\\u{7}"));
    }

    #[test]
    fn human_disposition_classifies_or_hides_agent_findings() {
        let mut store = ReviewCommentStore::default();
        let ids = store
            .insert_agent_batch(
                vec![NewAgentComment {
                    anchor: anchor(),
                    summary: "finding".to_owned(),
                    rationale: None,
                    author: None,
                }],
                1,
            )
            .unwrap();

        store
            .set_disposition(&ids[0], FindingDisposition::Blocking)
            .unwrap();
        assert!(store.label(&anchor()).unwrap().contains("blocking"));
        store
            .set_disposition(&ids[0], FindingDisposition::Dismissed)
            .unwrap();
        assert!(!store.contains_key(&anchor()));
        assert_eq!(store.len(), 1);
        store
            .set_disposition(&ids[0], FindingDisposition::Open)
            .unwrap();
        assert!(store.contains_key(&anchor()));
    }

    #[test]
    fn equivalent_range_sides_share_one_rendered_anchor() {
        let mut store = ReviewCommentStore::default();
        let human_anchor = range_anchor(AnnotationSide::Old, 37);
        store
            .insert_human(human_anchor.clone(), "question".to_owned(), 1)
            .unwrap();
        store
            .insert_agent_batch(
                vec![NewAgentComment {
                    anchor: range_anchor(AnnotationSide::New, 45),
                    summary: "answer".to_owned(),
                    rationale: None,
                    author: Some("agent".to_owned()),
                }],
                1,
            )
            .unwrap();

        assert_eq!(store.anchor_count(), 1);
        assert!(store.has_human(&human_anchor));
        assert!(store.has_agent(&human_anchor));
        assert!(
            store
                .get(&human_anchor)
                .unwrap()
                .contains("Human: question")
        );
        assert!(
            store
                .get(&human_anchor)
                .unwrap()
                .contains("Agent (agent): answer")
        );
    }

    #[test]
    fn mixed_anchor_removal_preserves_the_human_comment_until_agents_are_gone() {
        let mut store = ReviewCommentStore::default();
        let anchor = anchor();
        store
            .insert_human(anchor.clone(), "human note".to_owned(), 1)
            .unwrap();
        store
            .insert_agent_batch(
                vec![NewAgentComment {
                    anchor: anchor.clone(),
                    summary: "agent answer".to_owned(),
                    rationale: None,
                    author: None,
                }],
                1,
            )
            .unwrap();

        assert!(store.remove(&anchor).is_some());
        assert_eq!(store.human_text(&anchor), Some("human note"));
        assert!(store.is_human_only(&anchor));
        assert!(store.remove(&anchor).is_some());
        assert!(!store.contains_key(&anchor));
    }

    #[test]
    fn comments_reject_paths_outside_the_restore_limits() {
        let mut store = ReviewCommentStore::default();
        let mut invalid = anchor();
        invalid.path = "x".repeat(mark_session::MAX_PATH_BYTES + 1);

        assert_eq!(
            store.insert_human(invalid.clone(), "human note".to_owned(), 1),
            Err(StoreLimitError)
        );
        assert_eq!(
            store.insert_agent_batch(
                vec![NewAgentComment {
                    anchor: invalid,
                    summary: "agent note".to_owned(),
                    rationale: None,
                    author: None,
                }],
                1,
            ),
            Err(StoreLimitError)
        );
        assert!(store.is_empty());
    }

    #[test]
    fn agent_removal_cannot_remove_human_comment() {
        let mut store = ReviewCommentStore::default();
        store
            .insert_human(anchor(), "human note".to_owned(), 1)
            .unwrap();

        assert_eq!(store.remove_agent_by_id("human-1"), Err(()));
        assert_eq!(store.len(), 1);
    }
}
