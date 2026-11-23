use anyhow::{Result, bail, ensure};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Version of the seeded ordering. Change it when the same inputs can produce another order.
pub const ALGORITHM_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Random,
    Balanced,
    Intelligent,
}

impl Strategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Random => "random",
            Self::Balanced => "balanced",
            Self::Intelligent => "intelligent",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "random" => Ok(Self::Random),
            "balanced" => Ok(Self::Balanced),
            "intelligent" => Ok(Self::Intelligent),
            _ => bail!("Strategia di estrazione sconosciuta: {value}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub id: String,
    pub topic_id: i64,
    pub due_at: Option<i64>,
    pub wrong_count: u32,
}

/// Selects a versioned, seeded order from the eligible topics.
/// Timestamps for `now` and `due_at` must use the same units.
pub fn select(
    candidates: &[Candidate],
    topics: &[i64],
    count: usize,
    strategy: Strategy,
    seed: u64,
    now: i64,
) -> Result<Vec<String>> {
    ensure!(count > 0, "Scegli almeno una domanda");
    ensure!(!topics.is_empty(), "Seleziona almeno un argomento");

    let topics: BTreeSet<_> = topics.iter().copied().collect();
    let mut population: Vec<_> = candidates
        .iter()
        .filter(|candidate| topics.contains(&candidate.topic_id))
        .collect();
    let available_topics: BTreeSet<_> = population.iter().map(|item| item.topic_id).collect();
    if let Some(topic_id) = topics.difference(&available_topics).next() {
        bail!("Argomento senza domande disponibili: {topic_id}");
    }

    // Use the same canonical representative even if a repeated ID has different metadata.
    population
        .sort_unstable_by_key(|item| (&item.id, item.topic_id, item.due_at, item.wrong_count));
    population.dedup_by(|left, right| left.id == right.id);
    let available_count = population.len();
    ensure!(
        count <= available_count,
        "Disponibili {available_count} domande per gli argomenti selezionati, richieste {count}"
    );

    let mut random = SeededRandom(seed);
    random.shuffle(&mut population);
    match strategy {
        Strategy::Random => {}
        Strategy::Balanced => return Ok(balanced(population, count, &mut random)),
        Strategy::Intelligent => {
            // Stable sorting preserves the seeded order for equal priorities.
            population.sort_by_key(|item| {
                let due = item.due_at.is_some_and(|due_at| due_at <= now);
                (Reverse(due), Reverse(item.wrong_count))
            });
        }
    }

    Ok(population
        .into_iter()
        .take(count)
        .map(|item| item.id.clone())
        .collect())
}

fn balanced(population: Vec<&Candidate>, count: usize, random: &mut SeededRandom) -> Vec<String> {
    let mut topics = BTreeMap::<_, VecDeque<_>>::new();
    for item in population {
        topics.entry(item.topic_id).or_default().push_back(item);
    }
    let mut topics: Vec<_> = topics.into_values().collect();
    random.shuffle(&mut topics);

    let mut remaining: VecDeque<_> = topics.into();
    let mut selected = Vec::with_capacity(count);
    while let Some(mut topic) = remaining.pop_front() {
        let item = topic.pop_front().expect("only nonempty topics are queued");
        selected.push(item.id.clone());
        if selected.len() == count {
            break;
        }
        if !topic.is_empty() {
            remaining.push_back(topic);
        }
    }
    selected
}

pub fn scaled_threshold(
    count: usize,
    official_count: usize,
    allowed_errors: usize,
) -> Result<usize> {
    ensure!(count > 0, "Scegli almeno una domanda");
    ensure!(
        official_count > 0 && allowed_errors <= official_count,
        "Soglia ufficiale non valida"
    );
    Ok((count as u128 * allowed_errors as u128 / official_count as u128) as usize)
}

struct SeededRandom(u64);

impl SeededRandom {
    // SplitMix64 supplies a platform-independent stream for Fisher–Yates.
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        value ^ (value >> 31)
    }

    fn shuffle<T>(&mut self, values: &mut [T]) {
        for index in (1..values.len()).rev() {
            let bound = (index + 1) as u64;
            // Discard the uneven part of the random range before taking a remainder.
            let minimum = bound.wrapping_neg() % bound;
            let chosen = loop {
                let value = self.next();
                if value >= minimum {
                    break (value % bound) as usize;
                }
            };
            values.swap(index, chosen);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn candidate(id: &str, topic_id: i64) -> Candidate {
        Candidate {
            id: id.to_owned(),
            topic_id,
            due_at: None,
            wrong_count: 0,
        }
    }

    #[test]
    fn parses_persisted_strategy_names() {
        for (name, strategy) in [
            ("random", Strategy::Random),
            ("balanced", Strategy::Balanced),
            ("intelligent", Strategy::Intelligent),
        ] {
            assert_eq!(Strategy::parse(name).unwrap(), strategy);
            assert_eq!(strategy.as_str(), name);
        }
        assert!(Strategy::parse("unknown").is_err());
    }

    #[test]
    fn rejects_empty_and_unknown_topics() {
        let candidates = [candidate("a", 1), candidate("b", 1)];
        assert!(select(&candidates, &[], 1, Strategy::Random, 42, 100).is_err());
        assert!(select(&candidates, &[1, 99], 1, Strategy::Random, 42, 100).is_err());
    }

    #[test]
    fn rejects_zero_or_more_questions_than_the_unique_filtered_pool() {
        let candidates = [candidate("a", 1), candidate("a", 1), candidate("b", 2)];
        assert!(select(&candidates, &[1], 0, Strategy::Random, 42, 100).is_err());
        assert!(select(&candidates, &[1], 2, Strategy::Random, 42, 100).is_err());
    }

    #[test]
    fn each_strategy_returns_unique_questions_from_selected_topics() {
        let candidates = [
            candidate("a", 1),
            candidate("a", 1),
            candidate("b", 1),
            candidate("c", 2),
            candidate("d", 3),
        ];
        for strategy in [Strategy::Random, Strategy::Balanced, Strategy::Intelligent] {
            let ids = select(&candidates, &[2, 1, 2], 3, strategy, 42, 100).unwrap();
            assert_eq!(ids.len(), 3);
            assert_eq!(
                ids.iter().map(String::as_str).collect::<BTreeSet<_>>(),
                BTreeSet::from(["a", "b", "c"]),
            );
        }
    }

    #[test]
    fn seeded_selection_is_independent_of_input_order() {
        let candidates = [
            candidate("f", 2),
            candidate("c", 2),
            candidate("d", 1),
            candidate("a", 1),
            candidate("e", 2),
            candidate("b", 1),
        ];
        let mut reversed = candidates.clone();
        reversed.reverse();
        for strategy in [Strategy::Random, Strategy::Balanced, Strategy::Intelligent] {
            let ids = select(&candidates, &[1, 2], 4, strategy, 42, 100).unwrap();
            assert_eq!(
                ids,
                select(&reversed, &[2, 1], 4, strategy, 42, 100).unwrap(),
            );
            assert_ne!(
                ids,
                select(&candidates, &[1, 2], 4, strategy, 43, 100).unwrap(),
            );
        }
    }

    #[test]
    fn replays_a_version_one_random_selection() {
        let candidates = [
            candidate("a", 1),
            candidate("b", 1),
            candidate("c", 1),
            candidate("d", 1),
            candidate("e", 1),
            candidate("f", 1),
        ];
        assert_eq!(ALGORITHM_VERSION, 1);
        assert_eq!(
            select(&candidates, &[1], 6, Strategy::Random, 42, 100).unwrap(),
            ["e", "d", "a", "c", "f", "b"],
        );
    }

    #[test]
    fn balanced_selection_visits_each_topic_then_skips_exhausted_topics() {
        let candidates = [
            candidate("a", 1),
            candidate("b", 1),
            candidate("c", 1),
            candidate("d", 2),
            candidate("e", 2),
            candidate("f", 3),
        ];
        let ids = select(&candidates, &[1, 2, 3], 6, Strategy::Balanced, 42, 100).unwrap();
        let topics: Vec<_> = ids
            .iter()
            .map(|id| {
                candidates
                    .iter()
                    .find(|candidate| &candidate.id == id)
                    .unwrap()
                    .topic_id
            })
            .collect();
        assert_eq!(
            topics[..3].iter().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from([1, 2, 3])
        );
        assert_eq!(
            topics[3..5].iter().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from([1, 2])
        );
        assert_eq!(topics[5], 1);
    }

    #[test]
    fn intelligent_selection_prioritizes_due_reviews_then_wrong_answers() {
        let mut forgotten = candidate("forgotten", 1);
        forgotten.due_at = Some(100);
        forgotten.wrong_count = 3;
        let mut due = candidate("due", 2);
        due.due_at = Some(90);
        let mut incorrect = candidate("incorrect", 2);
        incorrect.due_at = Some(101);
        incorrect.wrong_count = 5;
        let candidates = [candidate("new", 1), due, incorrect, forgotten];
        assert_eq!(
            select(&candidates, &[1, 2], 4, Strategy::Intelligent, 42, 100).unwrap(),
            ["forgotten", "due", "incorrect", "new"],
        );
    }

    #[test]
    fn scales_the_threshold_using_floor_without_overflow() {
        assert_eq!(scaled_threshold(9, 30, 3).unwrap(), 0);
        assert_eq!(scaled_threshold(10, 30, 3).unwrap(), 1);
        assert_eq!(scaled_threshold(19, 30, 3).unwrap(), 1);
        assert_eq!(scaled_threshold(30, 30, 3).unwrap(), 3);
        assert_eq!(scaled_threshold(55, 30, 3).unwrap(), 5);
        assert_eq!(scaled_threshold(55, 30, 0).unwrap(), 0);
        assert_eq!(
            scaled_threshold(usize::MAX, usize::MAX, usize::MAX).unwrap(),
            usize::MAX
        );
    }

    #[test]
    fn rejects_invalid_threshold_inputs() {
        assert!(scaled_threshold(0, 30, 3).is_err());
        assert!(scaled_threshold(20, 0, 0).is_err());
        assert!(scaled_threshold(20, 30, 31).is_err());
    }
}
