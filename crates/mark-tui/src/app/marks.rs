use super::{DiffApp, MarkExport, MarkScope};
use crate::annotation::{
    AnnotationKey, AnnotationScope, AnnotationSide, paired_old_line_for_addition,
};
use crate::model::{UiModel, UiModelBuildOptions, UiRow, line_after_hunk};
use crate::syntax::{DiffSide, available_context_lines};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

#[derive(Serialize)]
struct MarksExport<'a> {
    version: u32,
    marks: &'a [MarkExport],
}

impl DiffApp {
    pub(crate) fn marks_clipboard_json(&self) -> Option<String> {
        let mut marks = self.export_marks();
        if marks.is_empty() {
            return None;
        }
        marks.sort_by(|left, right| {
            (
                &left.path,
                left.scope,
                left.old_line,
                left.new_line,
                left.old_start,
                left.new_start,
            )
                .cmp(&(
                    &right.path,
                    right.scope,
                    right.old_line,
                    right.new_line,
                    right.old_start,
                    right.new_start,
                ))
        });

        let export = MarksExport {
            version: 1,
            marks: &marks,
        };
        Some(serde_json::to_string_pretty(&export).expect("marks export should serialize"))
    }

    fn export_marks(&self) -> Vec<MarkExport> {
        // Copy marks for the current diff, not stale annotations whose path still
        // exists after a reload. Build an unfiltered model so active file/grep
        // filters do not hide otherwise-current marks from export.
        let export_model = UiModel::new_with_trailing_context_controls_and_annotation_candidates(
            &self.document.changeset,
            self.viewport.layout,
            &self.document.context_expansions,
            &HashMap::new(),
            UiModelBuildOptions::new(true, true, false),
        );
        let exportable_keys = self.exportable_annotation_keys(&export_model);
        self.annotations_state
            .annotations
            .keys()
            .filter_map(|key| {
                let body = self.annotations_state.annotations.human_bodies(key)?;
                if !exportable_keys.contains(key)
                    && !self.collapsed_context_contains_annotation_key(&export_model, key)
                {
                    return None;
                }
                Some(self.export_mark(key, &body))
            })
            .collect()
    }

    fn exportable_annotation_keys(&self, model: &UiModel) -> HashSet<AnnotationKey> {
        model
            .iter_rows()
            .flat_map(|row| AnnotationKey::candidates_from_ui_row(&self.document.changeset, row))
            .collect()
    }

    fn collapsed_context_contains_annotation_key(
        &self,
        model: &UiModel,
        key: &AnnotationKey,
    ) -> bool {
        key.is_line()
            && key.side == AnnotationSide::New
            && self.collapsed_context_contains_coordinate(model, &key.path, key.side, key.line)
    }

    fn collapsed_context_contains_coordinate(
        &self,
        model: &UiModel,
        path: &str,
        side: AnnotationSide,
        line: usize,
    ) -> bool {
        model.iter_rows().any(|row| {
            let UiRow::Collapsed {
                file,
                old_start,
                new_start,
                lines,
                ..
            } = row
            else {
                return false;
            };
            let Some(file) = self.document.changeset.files.get(file.get()) else {
                return false;
            };
            if AnnotationKey::path_for_side(file, side) != Some(path) {
                return false;
            }

            let start = match side {
                AnnotationSide::Old => old_start as usize,
                AnnotationSide::New => new_start as usize,
            };
            let lines = lines as usize;
            line >= start && line.saturating_sub(start) < lines
        }) || self.trailing_context_contains_coordinate(path, side, line)
    }

    fn trailing_context_contains_coordinate(
        &self,
        path: &str,
        coordinate_side: AnnotationSide,
        line: usize,
    ) -> bool {
        // The trailing control is discovered lazily for visible files. Keep a
        // source-derived fallback so marks export correctly before discovery.
        self.document
            .changeset
            .files
            .iter()
            .enumerate()
            .any(|(file_index, file)| {
                if AnnotationKey::path_for_side(file, coordinate_side) != Some(path) {
                    return false;
                }
                let Some(last_hunk) = file.hunks().last() else {
                    return false;
                };
                let old_start = line_after_hunk(last_hunk.old_start(), last_hunk.old_count());
                let new_start = line_after_hunk(last_hunk.new_start(), last_hunk.new_count());
                let coordinate_start = match coordinate_side {
                    AnnotationSide::Old => old_start,
                    AnnotationSide::New => new_start,
                };
                if line < coordinate_start {
                    return false;
                }

                let Some((source_side, source_line_count)) =
                    self.context_source_line_count(file_index)
                else {
                    return false;
                };
                let source_start = match source_side {
                    DiffSide::Old => old_start,
                    DiffSide::New => new_start,
                };
                let available =
                    available_context_lines(source_start, usize::MAX, source_line_count);

                line.saturating_sub(coordinate_start) < available
            })
    }

    fn export_mark(&self, key: &AnnotationKey, body: &str) -> MarkExport {
        let mut mark = MarkExport {
            path: key.path.clone(),
            scope: None,
            old_line: None,
            new_line: None,
            old_start: None,
            old_count: None,
            new_start: None,
            new_count: None,
            body: body.to_owned(),
        };
        match key.scope {
            AnnotationScope::File => mark.scope = Some(MarkScope::File),
            AnnotationScope::Hunk {
                old_start,
                old_count,
                new_start,
                new_count,
            } => {
                mark.scope = Some(MarkScope::Hunk);
                mark.old_start = Some(old_start);
                mark.old_count = Some(old_count);
                mark.new_start = Some(new_start);
                mark.new_count = Some(new_count);
            }
            AnnotationScope::Line => match key.side {
                AnnotationSide::Old => mark.old_line = Some(key.line),
                AnnotationSide::New => {
                    mark.old_line = self.paired_old_line_for_new_annotation(key);
                    mark.new_line = Some(key.line);
                }
            },
        }
        mark
    }

    fn paired_old_line_for_new_annotation(&self, key: &AnnotationKey) -> Option<usize> {
        self.document.changeset.files.iter().find_map(|file| {
            if AnnotationKey::path_for_side(file, AnnotationSide::New) != Some(key.path.as_str()) {
                return None;
            }

            file.hunks().iter().find_map(|hunk| {
                hunk.lines
                    .iter()
                    .enumerate()
                    .find_map(|(line_index, line)| {
                        if line.new_line() == Some(key.line) {
                            paired_old_line_for_addition(&hunk.lines, line_index)
                        } else {
                            None
                        }
                    })
            })
        })
    }
}
