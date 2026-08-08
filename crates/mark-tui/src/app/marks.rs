use super::{DiffApp, MarkExport, MarkScope, json_string};
use crate::annotation::{
    AnnotationKey, AnnotationScope, AnnotationSide, paired_old_line_for_addition,
};
use crate::model::{UiModel, UiModelBuildOptions, UiRow, line_after_hunk};
use crate::syntax::{DiffSide, available_context_lines};
use std::collections::{HashMap, HashSet};

#[cfg(test)]
use super::write_osc52_clipboard;
#[cfg(test)]
use std::io::Write;

impl DiffApp {
    #[cfg(test)]
    pub(crate) fn copy_marks_to_writer<W: Write>(&mut self, writer: &mut W) {
        let Some(marks) = self.marks_clipboard_json() else {
            self.set_warning_notice("no marks to copy");
            return;
        };

        match write_osc52_clipboard(writer, &marks) {
            Ok(()) => self.set_success_notice("marks copied"),
            Err(error) => self.set_error_log(format!("marks copy failed: {error}")),
        }
    }

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

        let mut out = String::from("{\n  \"version\": 1,\n  \"marks\": [\n");
        for (index, mark) in marks.iter().enumerate() {
            if index > 0 {
                out.push_str(",\n");
            }
            out.push_str("    {\n");
            out.push_str("      \"path\": ");
            out.push_str(&json_string(&mark.path));
            if let Some(scope) = mark.scope {
                out.push_str(",\n      \"scope\": ");
                out.push_str(&json_string(match scope {
                    MarkScope::File => "file",
                    MarkScope::Hunk => "hunk",
                    MarkScope::Range => "range",
                }));
            }
            if let Some(old_line) = mark.old_line {
                out.push_str(",\n      \"old_line\": ");
                out.push_str(&old_line.to_string());
            }
            if let Some(new_line) = mark.new_line {
                out.push_str(",\n      \"new_line\": ");
                out.push_str(&new_line.to_string());
            }
            for (name, value) in [
                ("old_start", mark.old_start),
                ("old_count", mark.old_count),
                ("new_start", mark.new_start),
                ("new_count", mark.new_count),
            ] {
                if let Some(value) = value {
                    out.push_str(",\n      \"");
                    out.push_str(name);
                    out.push_str("\": ");
                    out.push_str(&value.to_string());
                }
            }
            out.push_str(",\n      \"body\": ");
            out.push_str(&json_string(&mark.body));
            out.push_str("\n    }");
        }
        out.push_str("\n  ]\n}");
        Some(out)
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
        let exportable_range_coordinates = self.exportable_range_coordinates(&export_model);
        self.annotations_state
            .annotations
            .iter()
            .filter_map(|(key, body)| {
                let is_exportable = if key.is_range() {
                    self.range_is_exportable(&export_model, &exportable_range_coordinates, key)
                } else {
                    exportable_keys.contains(key)
                        || self.collapsed_context_contains_annotation_key(&export_model, key)
                };
                if !is_exportable {
                    return None;
                }
                self.export_mark(key, body)
            })
            .collect()
    }

    fn exportable_annotation_keys(&self, model: &UiModel) -> HashSet<AnnotationKey> {
        model
            .iter_rows()
            .flat_map(|row| AnnotationKey::candidates_from_ui_row(&self.document.changeset, row))
            .collect()
    }

    fn exportable_range_coordinates(
        &self,
        model: &UiModel,
    ) -> HashSet<(usize, AnnotationSide, usize)> {
        let mut coordinates = HashSet::new();
        for (model_row, row) in model.iter_rows().enumerate() {
            let Some(file_index) = model.file_at_row(model_row) else {
                continue;
            };
            for preferred_side in [AnnotationSide::Old, AnnotationSide::New] {
                if let Some((side, line)) = AnnotationKey::line_coordinates_from_ui_row(
                    &self.document.changeset,
                    row,
                    preferred_side,
                ) {
                    coordinates.insert((file_index, side, line));
                }
            }
            if let UiRow::UnifiedLine { file, hunk, line } = row
                && let Some(diff_line) = self
                    .document
                    .changeset
                    .files
                    .get(file.get())
                    .and_then(|file| file.hunks().get(hunk.get()))
                    .and_then(|hunk| hunk.lines.get(line.get()))
            {
                if let Some(line) = diff_line.old_line() {
                    coordinates.insert((file_index, AnnotationSide::Old, line));
                }
                if let Some(line) = diff_line.new_line() {
                    coordinates.insert((file_index, AnnotationSide::New, line));
                }
            }
        }
        coordinates
    }

    fn range_is_exportable(
        &self,
        model: &UiModel,
        exportable_coordinates: &HashSet<(usize, AnnotationSide, usize)>,
        key: &AnnotationKey,
    ) -> bool {
        let AnnotationScope::Range {
            old_start,
            old_count,
            new_start,
            new_count,
        } = key.scope
        else {
            return false;
        };
        if !key.covers_coordinate(key.side, key.line) {
            return false;
        }

        self.document
            .changeset
            .files
            .iter()
            .enumerate()
            .filter(|(_, file)| {
                AnnotationKey::path_for_side(file, key.side) == Some(key.path.as_str())
            })
            .any(|(file_index, file)| {
                self.source_range_is_exportable(
                    model,
                    exportable_coordinates,
                    file_index,
                    file,
                    AnnotationSide::Old,
                    old_start,
                    old_count,
                ) && self.source_range_is_exportable(
                    model,
                    exportable_coordinates,
                    file_index,
                    file,
                    AnnotationSide::New,
                    new_start,
                    new_count,
                )
            })
    }

    #[allow(clippy::too_many_arguments)]
    fn source_range_is_exportable(
        &self,
        model: &UiModel,
        exportable_coordinates: &HashSet<(usize, AnnotationSide, usize)>,
        file_index: usize,
        file: &mark_diff::DiffFile,
        side: AnnotationSide,
        start: usize,
        count: usize,
    ) -> bool {
        if count == 0 {
            return true;
        }
        let Some(path) = AnnotationKey::path_for_side(file, side) else {
            return false;
        };

        (0..count).all(|offset| {
            let Some(line) = start.checked_add(offset) else {
                return false;
            };
            exportable_coordinates.contains(&(file_index, side, line))
                || self.collapsed_context_contains_coordinate(model, path, side, line)
        })
    }

    fn collapsed_context_contains_annotation_key(
        &self,
        model: &UiModel,
        key: &AnnotationKey,
    ) -> bool {
        (key.is_line() || key.is_range())
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

    fn export_mark(&self, key: &AnnotationKey, body: &str) -> Option<MarkExport> {
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
            AnnotationScope::Range {
                old_start,
                old_count,
                new_start,
                new_count,
            } => {
                mark.scope = Some(MarkScope::Range);
                if old_count > 0 {
                    mark.old_start = Some(old_start);
                    mark.old_count = Some(old_count);
                }
                if new_count > 0 {
                    mark.new_start = Some(new_start);
                    mark.new_count = Some(new_count);
                }
            }
            AnnotationScope::Line => match key.side {
                AnnotationSide::Old => mark.old_line = Some(key.line),
                AnnotationSide::New => {
                    mark.old_line = self.paired_old_line_for_new_annotation(key);
                    mark.new_line = Some(key.line);
                }
            },
        }
        Some(mark)
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
