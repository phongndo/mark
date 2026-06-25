use std::collections::HashMap;

use mark_diff::{Changeset, DiffLine};

use crate::model::UiRow;

pub(crate) const ANNOTATION_ADD_BUTTON: &str = " [+]";
pub(crate) const ANNOTATION_ADD_BUTTON_WIDTH: usize = 4;
pub(crate) const ANNOTATION_CLOSE_BUTTON: &str = "[x]";
pub(crate) const ANNOTATION_CLOSE_BUTTON_WIDTH: usize = 3;
pub(crate) const ANNOTATION_SUBMIT_BUTTON: &str = "[✓]";
pub(crate) const ANNOTATION_SUBMIT_BUTTON_ASCII: &str = "[s]";
pub(crate) const ANNOTATION_SUBMIT_BUTTON_WIDTH: usize = 3;
pub(crate) const ANNOTATION_EDIT_BUTTON: &str = "[↻]";
pub(crate) const ANNOTATION_EDIT_BUTTON_ASCII: &str = "[e]";
pub(crate) const ANNOTATION_EDIT_BUTTON_WIDTH: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct AnnotationKey {
    pub(crate) path: String,
    pub(crate) side: AnnotationSide,
    pub(crate) line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum AnnotationSide {
    Old,
    New,
}

impl AnnotationSide {
    pub(crate) fn label(self) -> char {
        match self {
            Self::Old => 'L',
            Self::New => 'R',
        }
    }
}

impl AnnotationKey {
    pub(crate) fn from_ui_row(changeset: &Changeset, row: UiRow) -> Option<Self> {
        match row {
            UiRow::UnifiedLine { file, hunk, line } | UiRow::MetaLine { file, hunk, line } => {
                let file = changeset.files.get(file)?;
                let diff_line = file.hunks.get(hunk)?.lines.get(line)?;
                Self::from_diff_line(file.display_path(), diff_line)
            }
            UiRow::SplitLine {
                file,
                hunk,
                left,
                right,
            } => {
                let file = changeset.files.get(file)?;
                let lines = &file.hunks.get(hunk)?.lines;
                if let Some(index) = right {
                    let line = lines.get(index)?.new_line?;
                    return Some(Self::new(file.display_path(), AnnotationSide::New, line));
                }
                let index = left?;
                let line = lines.get(index)?.old_line?;
                Some(Self::new(file.display_path(), AnnotationSide::Old, line))
            }
            UiRow::ContextLine { file, new_line, .. } => {
                let file = changeset.files.get(file)?;
                Some(Self::new(
                    file.display_path(),
                    AnnotationSide::New,
                    new_line,
                ))
            }
            _ => None,
        }
    }

    fn from_diff_line(path: &str, line: &DiffLine) -> Option<Self> {
        line.new_line
            .map(|line| Self::new(path, AnnotationSide::New, line))
            .or_else(|| {
                line.old_line
                    .map(|line| Self::new(path, AnnotationSide::Old, line))
            })
    }

    fn new(path: &str, side: AnnotationSide, line: usize) -> Self {
        Self {
            path: path.to_owned(),
            side,
            line,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AnnotationDraft {
    pub(crate) key: AnnotationKey,
    pub(crate) model_row_index: usize,
    pub(crate) input: String,
    pub(crate) cursor: usize,
}

pub(crate) type AnnotationStore = HashMap<AnnotationKey, String>;
