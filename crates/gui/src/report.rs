//! Browse full job reports without laying out every note on each repaint.
use anynle_app::job::{ReportNote, ReviewGroup};
use std::collections::HashMap;
use std::ops::Range;

pub(crate) const PAGE_SIZE: usize = 25;

fn contains_lowercase(text: &mut str, query: &str) -> bool {
    if text.is_ascii() {
        text.make_ascii_lowercase();
        text.contains(query)
    } else {
        text.to_lowercase().contains(query)
    }
}

fn candidate_indices(timeline: Option<&ReviewGroup>, total: usize) -> impl Iterator<Item = usize> + '_ {
    (0..timeline.map_or(total, |group| group.note_indices.len()))
        .map(move |index| timeline.map_or(index, |group| group.note_indices[index]))
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Filter {
    #[default]
    All,
    Changes,
    Warnings,
}

#[derive(Default)]
pub(crate) struct ReportBrowser {
    pub(crate) search: String,
    pub(crate) filter: Filter,
    pub(crate) timeline: Option<usize>,
    pub(crate) source_clip: Option<usize>,
    pub(crate) page: usize,
    pub(crate) changes: usize,
    pub(crate) warnings: usize,
    pub(crate) change_examples: Vec<usize>,
    matches: Option<Vec<usize>>,
}

impl ReportBrowser {
    pub(crate) fn new(notes: &[ReportNote]) -> Self {
        let mut browser = Self::default();
        for (index, note) in notes.iter().enumerate() {
            match note.severity.as_str() {
                "loss" => {
                    browser.changes += 1;
                    if browser.change_examples.len() < 3 {
                        browser.change_examples.push(index);
                    }
                }
                "warning" => browser.warnings += 1,
                _ => {}
            }
        }
        browser
    }

    pub(crate) fn refilter(
        &mut self,
        notes: &[ReportNote],
        subjects: Option<&HashMap<String, String>>,
        timelines: &[ReviewGroup],
        sources: &[ReviewGroup],
    ) {
        self.page = 0;
        let query = self.search.trim().to_lowercase();
        let timeline = self.timeline.and_then(|index| timelines.get(index));
        let source = self.source_clip.and_then(|index| sources.get(index));
        let group = timeline.or(source);
        let intersection = timeline.and(source);
        if group.is_none() && query.is_empty() && self.filter == Filter::All {
            self.matches = None;
            return;
        }
        let mut searchable = String::new();
        self.matches = Some(
            candidate_indices(group, notes.len())
                .filter(|index| intersection.is_none_or(|source|
                    source.note_indices.binary_search(index).is_ok()))
                .filter(|&index| {
                    let note = &notes[index];
                    let severity_matches = match self.filter {
                        Filter::All => true,
                        Filter::Changes => note.severity == "loss",
                        Filter::Warnings => note.severity == "warning",
                    };
                    if !severity_matches {
                        return false;
                    }
                    if query.is_empty() {
                        return true;
                    }
                    let subject = subjects
                        .and_then(|names| names.get(&note.object))
                        .map_or("", String::as_str);
                    searchable.clear();
                    searchable.push_str(&note.severity);
                    for field in [&note.object, &note.message, subject, &note.why] {
                        searchable.push(' ');
                        searchable.push_str(field);
                    }
                    contains_lowercase(&mut searchable, &query)
                })
                .collect(),
        );
    }

    pub(crate) fn refilter_details(&mut self, details: &[String], timelines: &[ReviewGroup]) {
        self.page = 0;
        let query = self.search.trim().to_lowercase();
        let timeline = self.timeline.and_then(|index| timelines.get(index));
        self.matches = (timeline.is_some() || !query.is_empty()).then(|| {
            let mut text = String::new();
            candidate_indices(timeline, details.len()).filter(|&index| {
                text.clear();
                text.push_str(&details[index]);
                query.is_empty() || contains_lowercase(&mut text, &query)
            }).collect()
        });
    }

    pub(crate) fn count(&self, notes_len: usize) -> usize {
        self.matches.as_ref().map_or(notes_len, Vec::len)
    }

    pub(crate) fn page_range(&self, notes_len: usize) -> Range<usize> {
        let count = self.count(notes_len);
        let start = (self.page * PAGE_SIZE).min(count);
        start..(start + PAGE_SIZE).min(count)
    }

    pub(crate) fn page_indices(&self, notes_len: usize) -> impl Iterator<Item = usize> + '_ {
        self.page_range(notes_len).map(|index| {
            self.matches
                .as_ref()
                .map_or(index, |matches| matches[index])
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_report_shows_first_middle_and_last_page_in_order() {
        let total = 445_003;
        let mut browser = ReportBrowser::default();
        assert_eq!(
            browser.page_indices(total).collect::<Vec<_>>(),
            (0..25).collect::<Vec<_>>()
        );
        browser.page = 259;
        assert_eq!(browser.page_range(total), 6_475..6_500);
        browser.page = (total - 1) / PAGE_SIZE;
        assert_eq!(
            browser.page_indices(total).collect::<Vec<_>>(),
            vec![445_000, 445_001, 445_002]
        );
    }

    #[test]
    fn filters_search_all_pages_by_severity_and_subject() {
        let notes: Vec<_> = (0..81)
            .map(|index| ReportNote {
                severity: if index % 2 == 0 { "loss" } else { "warning" }.into(),
                object: index.to_string(),
                message: "Edit needs attention".into(),
                why: if index == 80 {
                    "timewarp_gap".into()
                } else {
                    String::new()
                },
            })
            .collect();
        let names = HashMap::from([("78".into(), "Café".into()), ("77".into(), "CAFÉ".into())]);
        let mut browser = ReportBrowser::new(&notes);
        assert_eq!((browser.changes, browser.warnings), (41, 40));
        assert_eq!(browser.change_examples, vec![0, 2, 4]);
        browser.page = 3;
        browser.search = " CAFÉ ".into();
        browser.refilter(&notes, Some(&names), &[], &[]);
        assert_eq!(browser.page, 0);
        assert_eq!(
            browser.page_indices(notes.len()).collect::<Vec<_>>(),
            vec![77, 78]
        );
        browser.filter = Filter::Changes;
        browser.refilter(&notes, Some(&names), &[], &[]);
        assert_eq!(
            browser.page_indices(notes.len()).collect::<Vec<_>>(),
            vec![78]
        );
        browser.filter = Filter::Warnings;
        browser.refilter(&notes, Some(&names), &[], &[]);
        assert_eq!(
            browser.page_indices(notes.len()).collect::<Vec<_>>(),
            vec![77]
        );
        browser.search = "no match".into();
        browser.refilter(&notes, Some(&names), &[], &[]);
        assert_eq!(browser.count(notes.len()), 0);
        browser.filter = Filter::All;
        browser.search = "TIMEWARP_GAP".into();
        browser.refilter(&notes, Some(&names), &[], &[]);
        assert_eq!(
            browser.page_indices(notes.len()).collect::<Vec<_>>(),
            vec![80]
        );
        browser.search.clear();
        browser.refilter(&notes, Some(&names), &[], &[]);
        assert_eq!(browser.count(notes.len()), 81);
        let timelines = [
            ReviewGroup {
                id: "seq-a".into(), name: "Assembly".into(),
                bin: Some("First/Scenes.avb".into()), note_indices: vec![76, 77],
            },
            ReviewGroup {
                id: "seq-b".into(), name: "Assembly".into(),
                bin: Some("Second/Scenes.avb".into()), note_indices: vec![78, 79, 80],
            },
        ];
        browser.timeline = Some(1);
        browser.refilter(&notes, Some(&names), &timelines, &[]);
        assert_eq!(browser.page_indices(notes.len()).collect::<Vec<_>>(), [78, 79, 80]);
        browser.filter = Filter::Warnings;
        browser.refilter(&notes, Some(&names), &timelines, &[]);
        assert_eq!(browser.page_indices(notes.len()).collect::<Vec<_>>(), [79]);
        browser.filter = Filter::All;
        browser.search = "CAfé".into();
        browser.refilter(&notes, Some(&names), &timelines, &[]);
        assert_eq!(browser.page_indices(notes.len()).collect::<Vec<_>>(), [78]);
        let sources = [ReviewGroup {
            id: "clip".into(), name: "Camera".into(), bin: None,
            note_indices: vec![76, 78, 79],
        }];
        browser.source_clip = Some(0);
        browser.search.clear();
        browser.refilter(&notes, Some(&names), &timelines, &sources);
        assert_eq!(browser.page_indices(notes.len()).collect::<Vec<_>>(), [78, 79]);
        browser.filter = Filter::Warnings;
        browser.refilter(&notes, Some(&names), &timelines, &sources);
        assert_eq!(browser.page_indices(notes.len()).collect::<Vec<_>>(), [79]);
        browser.filter = Filter::All;
        browser.timeline = None;
        browser.refilter(&notes, Some(&names), &timelines, &sources);
        assert_eq!(browser.page_indices(notes.len()).collect::<Vec<_>>(), [76, 78, 79]);
    }
}
