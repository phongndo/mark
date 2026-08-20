use std::collections::{HashMap, HashSet};

use crate::annotation::AnnotationKey;

use super::comment::{
    CommentLifecycle, CommentOrigin, FindingDisposition, NewAgentComment, ReviewComment,
};

const MAX_LIVE_REVIEW_BYTES: usize = 2 * 1024 * 1024;
const COMMENT_OVERHEAD_BYTES: usize = 256;

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

    fn live_review_bytes(&self) -> usize {
        self.comments()
            .map(review_comment_bytes)
            .fold(0usize, usize::saturating_add)
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
        self.human_index(key).and_then(|index| {
            self.comments
                .get(index)?
                .as_ref()
                .map(|comment| comment.summary.as_str())
        })
    }

    pub(crate) fn editable_human_text(&self, key: &AnnotationKey) -> Option<&str> {
        self.last_comment_is_human(key)
            .then(|| self.human_text(key))
            .flatten()
    }

    pub(crate) fn last_comment_is_human(&self, key: &AnnotationKey) -> bool {
        self.last_comment(key)
            .is_some_and(|comment| comment.origin == CommentOrigin::Human)
    }

    pub(crate) fn human_bodies(&self, key: &AnnotationKey) -> Option<String> {
        let bodies = self
            .comments_at(key)
            .filter(|comment| comment.origin == CommentOrigin::Human)
            .map(|comment| comment.summary.as_str())
            .collect::<Vec<_>>();
        if bodies.is_empty() {
            None
        } else {
            Some(bodies.join("\n\n"))
        }
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

    pub(crate) fn insert_human(
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
        // The latest human turn is editable. A new note after an agent answer
        // starts another turn.
        if self.last_comment_is_human(&anchor)
            && let Some(index) = self.human_index(&anchor)
        {
            let comment = self.comments[index]
                .as_ref()
                .expect("indexed comment should exist");
            let next_bytes = self
                .live_review_bytes()
                .saturating_sub(review_comment_bytes(comment))
                .saturating_add(review_comment_bytes_with_summary(comment, &text));
            if next_bytes > MAX_LIVE_REVIEW_BYTES {
                return Err(StoreLimitError);
            }
            let comment = self.comments[index]
                .as_mut()
                .expect("indexed comment should exist");
            let previous = std::mem::replace(&mut comment.summary, text);
            comment.document_generation = generation;
            self.rebuild_anchor(&anchor);
            return Ok(Some(previous));
        }
        let id = self.prospective_comment_id("human", 1);
        if self.comment_count >= mark_session::MAX_LIVE_COMMENTS
            || self
                .live_review_bytes()
                .saturating_add(new_comment_bytes(&id, &anchor, &text, None, None))
                > MAX_LIVE_REVIEW_BYTES
        {
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
            original_anchor_evidence: None,
        });
        Ok(None)
    }

    pub(crate) fn restore_comments(
        &mut self,
        comments: Vec<ReviewComment>,
    ) -> Result<(), StoreLimitError> {
        if comments.len() > mark_session::MAX_LIVE_COMMENTS
            || comments.iter().map(review_comment_bytes).sum::<usize>() > MAX_LIVE_REVIEW_BYTES
        {
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
            || self.live_review_bytes().saturating_add(
                comments
                    .iter()
                    .enumerate()
                    .map(|(index, comment)| {
                        let id = self.prospective_comment_id("agent", index.saturating_add(1));
                        new_comment_bytes(
                            &id,
                            &comment.anchor,
                            &comment.summary,
                            comment.rationale.as_deref(),
                            comment.author.as_deref(),
                        )
                    })
                    .sum::<usize>(),
            ) > MAX_LIVE_REVIEW_BYTES
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
                original_anchor_evidence: None,
            });
        }
        Ok(ids)
    }

    #[cfg(test)]
    pub(crate) fn remove(&mut self, anchor: &AnnotationKey) -> Option<String> {
        let previous = self.get(anchor).cloned()?;
        if self.has_agent(anchor) {
            self.remove_agents_at(anchor);
        } else {
            self.remove_human(anchor)?;
        }
        Some(previous)
    }

    pub(crate) fn clear_anchor(&mut self, anchor: &AnnotationKey) -> usize {
        let Some(ids) = self.anchors.get(anchor).map(|view| view.ids.clone()) else {
            return 0;
        };
        let count = ids.len();
        for id in ids {
            self.remove_any_by_id(&id);
        }
        count
    }

    pub(crate) fn clear_all(&mut self) -> usize {
        let count = self.comment_count;
        *self = Self::default();
        count
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

    #[cfg(test)]
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

    fn last_comment(&self, anchor: &AnnotationKey) -> Option<&ReviewComment> {
        let id = self.anchors.get(anchor)?.ids.last()?;
        let index = *self.by_id.get(id)?;
        self.comments.get(index)?.as_ref()
    }

    fn human_index(&self, anchor: &AnnotationKey) -> Option<usize> {
        self.anchors.get(anchor)?.ids.iter().rev().find_map(|id| {
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

    fn prospective_comment_id(&self, prefix: &str, offset: usize) -> String {
        let offset = u64::try_from(offset).unwrap_or(u64::MAX);
        comment_id(prefix, self.next_id.saturating_add(offset))
    }

    fn next_comment_id(&mut self, prefix: &str) -> String {
        let id = self.prospective_comment_id(prefix, 1);
        self.next_id = self.next_id.saturating_add(1);
        id
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

fn comment_id(prefix: &str, sequence: u64) -> String {
    format!("{prefix}-{sequence}")
}

fn new_comment_bytes(
    id: &str,
    anchor: &AnnotationKey,
    summary: &str,
    rationale: Option<&str>,
    author: Option<&str>,
) -> usize {
    COMMENT_OVERHEAD_BYTES
        .saturating_add(id.len())
        .saturating_add(anchor.path.len())
        .saturating_add(summary.len())
        .saturating_add(rationale.map_or(0, str::len))
        .saturating_add(author.map_or(0, str::len))
}

fn review_comment_bytes(comment: &ReviewComment) -> usize {
    review_comment_bytes_with_summary(comment, &comment.summary)
}

fn review_comment_bytes_with_summary(comment: &ReviewComment, summary: &str) -> usize {
    new_comment_bytes(
        &comment.id,
        &comment.anchor,
        summary,
        comment.rationale.as_deref(),
        comment.author.as_deref(),
    )
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
        render::annotations::render_annotation_saved_block,
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

    fn fill_with_max_human_comments(store: &mut ReviewCommentStore, count: usize) {
        let text = "x".repeat(mark_session::MAX_RATIONALE_BYTES);
        for line in 1..=count {
            let mut anchor = anchor();
            anchor.line = line;
            store.insert_human(anchor, text.clone(), 1).unwrap();
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
    fn human_comment_at_byte_limit_survives_restore() {
        let mut store = ReviewCommentStore::default();
        fill_with_max_human_comments(&mut store, 63);
        let baseline = store.comments().cloned().collect::<Vec<_>>();
        let mut final_anchor = anchor();
        final_anchor.line = 64;
        let final_id = store.prospective_comment_id("human", 1);
        let fixed_bytes = new_comment_bytes(&final_id, &final_anchor, "", None, None);
        let remaining = MAX_LIVE_REVIEW_BYTES - store.live_review_bytes();
        assert!(remaining > fixed_bytes);
        let text = "x".repeat(remaining - fixed_bytes);
        assert!(text.len() < mark_session::MAX_RATIONALE_BYTES);

        let mut oversized = ReviewCommentStore::default();
        oversized.restore_comments(baseline).unwrap();
        assert_eq!(
            oversized.insert_human(final_anchor.clone(), format!("{text}x"), 1),
            Err(StoreLimitError)
        );

        store.insert_human(final_anchor, text, 1).unwrap();
        assert_eq!(store.live_review_bytes(), MAX_LIVE_REVIEW_BYTES);
        let comments = store.comments().cloned().collect();
        let mut restored = ReviewCommentStore::default();
        restored.restore_comments(comments).unwrap();
        assert_eq!(restored.live_review_bytes(), MAX_LIVE_REVIEW_BYTES);
    }

    #[test]
    fn agent_batch_at_byte_limit_accounts_for_every_id_and_survives_restore() {
        let mut store = ReviewCommentStore::default();
        fill_with_max_human_comments(&mut store, 62);
        let baseline = store.comments().cloned().collect::<Vec<_>>();
        let remaining = MAX_LIVE_REVIEW_BYTES - store.live_review_bytes();
        let first_id = store.prospective_comment_id("agent", 1);
        let second_id = store.prospective_comment_id("agent", 2);
        let mut first_anchor = anchor();
        first_anchor.line = 63;
        let first = NewAgentComment {
            anchor: first_anchor,
            summary: "s".repeat(mark_session::MAX_SUMMARY_BYTES),
            rationale: Some("r".repeat(mark_session::MAX_RATIONALE_BYTES)),
            author: Some("a".repeat(mark_session::MAX_AUTHOR_BYTES)),
        };
        let mut second_anchor = anchor();
        second_anchor.line = 64;
        let second_fixed_bytes = new_comment_bytes(&second_id, &second_anchor, "", None, None);
        let first_bytes = new_comment_bytes(
            &first_id,
            &first.anchor,
            &first.summary,
            first.rationale.as_deref(),
            first.author.as_deref(),
        );
        assert!(remaining > first_bytes.saturating_add(second_fixed_bytes));
        let second_summary = "s".repeat(remaining - first_bytes.saturating_add(second_fixed_bytes));
        assert!(second_summary.len() < mark_session::MAX_SUMMARY_BYTES);
        let comments = vec![
            first,
            NewAgentComment {
                anchor: second_anchor,
                summary: second_summary,
                rationale: None,
                author: None,
            },
        ];

        let mut oversized = ReviewCommentStore::default();
        oversized.restore_comments(baseline).unwrap();
        let mut oversized_comments = comments.clone();
        oversized_comments[1].summary.push('x');
        assert_eq!(
            oversized.insert_agent_batch(oversized_comments, 1),
            Err(StoreLimitError)
        );

        let ids = store.insert_agent_batch(comments, 1).unwrap();
        assert_eq!(ids, vec![first_id, second_id]);
        assert_eq!(store.live_review_bytes(), MAX_LIVE_REVIEW_BYTES);
        let comments = store.comments().cloned().collect();
        let mut restored = ReviewCommentStore::default();
        restored.restore_comments(comments).unwrap();
        assert_eq!(restored.live_review_bytes(), MAX_LIVE_REVIEW_BYTES);
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

    #[test]
    fn human_follow_up_after_agent_appends_a_new_turn() {
        let mut store = ReviewCommentStore::default();
        store
            .insert_human(anchor(), "what does this mean?".to_owned(), 1)
            .unwrap();
        store
            .insert_agent_batch(
                vec![NewAgentComment {
                    anchor: anchor(),
                    summary: "it retries".to_owned(),
                    rationale: None,
                    author: Some("pi".to_owned()),
                }],
                1,
            )
            .unwrap();

        assert_eq!(store.human_text(&anchor()), Some("what does this mean?"));
        assert_eq!(store.editable_human_text(&anchor()), None);
        assert_eq!(
            store.insert_human(anchor(), "replace it".to_owned(), 1),
            Ok(None)
        );
        assert_eq!(store.len(), 3);
        assert_eq!(store.human_text(&anchor()), Some("replace it"));
        assert_eq!(store.editable_human_text(&anchor()), Some("replace it"));
        assert_eq!(
            store.human_bodies(&anchor()).as_deref(),
            Some("what does this mean?\n\nreplace it")
        );
        let rendered = store.get(&anchor()).expect("stacked mark");
        assert!(rendered.contains("Human: what does this mean?"));
        assert!(rendered.contains("Agent (pi): it retries"));
        assert!(rendered.contains("Human: replace it"));
    }

    #[test]
    fn editing_the_latest_human_turn_replaces_it() {
        let mut store = ReviewCommentStore::default();
        store
            .insert_human(anchor(), "what does this mean?".to_owned(), 1)
            .unwrap();
        store
            .insert_agent_batch(
                vec![NewAgentComment {
                    anchor: anchor(),
                    summary: "it retries".to_owned(),
                    rationale: None,
                    author: Some("pi".to_owned()),
                }],
                1,
            )
            .unwrap();
        store.insert_human(anchor(), "typo".to_owned(), 1).unwrap();

        assert_eq!(
            store.insert_human(anchor(), "fixed".to_owned(), 1),
            Ok(Some("typo".to_owned()))
        );
        assert_eq!(store.len(), 3);
        assert_eq!(store.human_text(&anchor()), Some("fixed"));
        assert_eq!(
            store.human_bodies(&anchor()).as_deref(),
            Some("what does this mean?\n\nfixed")
        );
    }

    #[test]
    fn human_only_edit_still_replaces() {
        let mut store = ReviewCommentStore::default();
        store.insert_human(anchor(), "typo".to_owned(), 1).unwrap();
        assert_eq!(
            store.insert_human(anchor(), "fixed".to_owned(), 1),
            Ok(Some("typo".to_owned()))
        );
        assert_eq!(store.len(), 1);
        assert_eq!(store.human_text(&anchor()), Some("fixed"));
    }

    #[test]
    fn clear_anchor_removes_the_whole_stack() {
        let mut store = ReviewCommentStore::default();
        store
            .insert_human(anchor(), "question".to_owned(), 1)
            .unwrap();
        store
            .insert_agent_batch(
                vec![NewAgentComment {
                    anchor: anchor(),
                    summary: "answer".to_owned(),
                    rationale: None,
                    author: None,
                }],
                1,
            )
            .unwrap();
        assert_eq!(store.clear_anchor(&anchor()), 2);
        assert!(store.is_empty());
    }
}
