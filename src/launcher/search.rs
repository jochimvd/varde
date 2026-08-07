use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{Atom, AtomKind, CaseMatching, Normalization},
};

use super::source::Item;
#[cfg(test)]
use super::source::Visual;

const TITLE_BONUS: u32 = 1_000;

pub(super) fn rank(items: &[Item], query: &str, alphabetical: bool) -> Vec<usize> {
    if query.is_empty() {
        let mut indexes = (0..items.len()).collect::<Vec<_>>();
        if alphabetical {
            indexes.sort_by(|left, right| item_order(&items[*left], &items[*right]));
        }
        return indexes;
    }

    let pattern = Atom::new(
        query,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
        false,
    );
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buffer = Vec::new();
    let mut matches = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let title = pattern
                .score(Utf32Str::new(&item.title, &mut buffer), &mut matcher)
                .map(|score| u32::from(score) + TITLE_BONUS);
            let metadata = item
                .search_terms
                .iter()
                .filter_map(|term| {
                    pattern
                        .score(Utf32Str::new(term, &mut buffer), &mut matcher)
                        .map(u32::from)
                })
                .max();
            title.max(metadata).map(|score| (index, score))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|(left_index, left_score), (right_index, right_score)| {
        right_score
            .cmp(left_score)
            .then_with(|| item_order(&items[*left_index], &items[*right_index]))
    });
    matches.into_iter().map(|(index, _)| index).collect()
}

fn item_order(left: &Item, right: &Item) -> std::cmp::Ordering {
    left.title
        .to_lowercase()
        .cmp(&right.title.to_lowercase())
        .then_with(|| left.id.cmp(&right.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, title: &str, terms: &[&str]) -> Item {
        Item {
            id: id.into(),
            title: title.into(),
            visual: Visual::None,
            search_terms: terms.iter().map(|term| (*term).into()).collect(),
        }
    }

    #[test]
    fn sorts_empty_app_queries_but_preserves_dmenu_order() {
        let items = vec![item("2", "Zulu", &[]), item("1", "Alpha", &[])];
        assert_eq!(rank(&items, "", true), vec![1, 0]);
        assert_eq!(rank(&items, "", false), vec![0, 1]);
    }

    #[test]
    fn title_matches_beat_metadata_matches() {
        let items = vec![
            item("browser", "Web", &["firefox"]),
            item("firefox", "Firefox", &[]),
        ];
        assert_eq!(rank(&items, "ff", true), vec![1, 0]);
    }

    #[test]
    fn fuzzy_matches_non_contiguous_characters() {
        let items = vec![item("firefox", "Firefox", &[])];
        assert_eq!(rank(&items, "frx", true), vec![0]);
    }
}
