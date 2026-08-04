use super::{DiffApp, MarkExport, MarkScope, json_string};
use crate::annotation::{
    AnnotationKey, AnnotationScope, AnnotationSide, paired_old_line_for_addition,
};
use crate::model::{UiModel, UiRow, line_after_hunk};
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
            true,
            false,
        );
        let exportable_keys = self.exportable_annotation_keys(&export_model);
        self.annotations_state
            .annotations
            .iter()
            .filter_map(|(key, body)| {
                if !exportable_keys.contains(key)
                    && !self.collapsed_context_contains_annotation_key(&export_model, key)
                {
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

    fn collapsed_context_contains_annotation_key(
        &self,
        model: &UiModel,
        key: &AnnotationKey,
    ) -> bool {
        if !key.is_line() || key.side != AnnotationSide::New {
            return false;
        }

        model.iter_rows().any(|row| {
            let UiRow::Collapsed {
                file,
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
            if AnnotationKey::path_for_side(file, AnnotationSide::New) != Some(key.path.as_str()) {
                return false;
            }

            let new_start = new_start as usize;
            let lines = lines as usize;
            key.line >= new_start && key.line.saturating_sub(new_start) < lines
        }) || self.trailing_context_contains_annotation_key(key)
    }

    fn trailing_context_contains_annotation_key(&self, key: &AnnotationKey) -> bool {
        if !key.is_line() {
            return false;
        }
        // The trailing control is discovered lazily for visible files. Keep a
        // source-derived fallback so marks export correctly before discovery.
        self.document
            .changeset
            .files
            .iter()
            .enumerate()
            .any(|(file_index, file)| {
                if AnnotationKey::path_for_side(file, AnnotationSide::New)
                    != Some(key.path.as_str())
                {
                    return false;
                }
                let Some(last_hunk) = file.hunks().last() else {
                    return false;
                };
                let old_start = line_after_hunk(last_hunk.old_start(), last_hunk.old_count());
                let new_start = line_after_hunk(last_hunk.new_start(), last_hunk.new_count());
                if key.line < new_start {
                    return false;
                }

                let Some((side, source_line_count)) = self.context_source_line_count(file_index)
                else {
                    return false;
                };
                let source_start = match side {
                    DiffSide::Old => old_start,
                    DiffSide::New => new_start,
                };
                let available =
                    available_context_lines(source_start, usize::MAX, source_line_count);

                key.line.saturating_sub(new_start) < available
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
