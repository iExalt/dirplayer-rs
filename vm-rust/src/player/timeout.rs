use std::collections::HashMap;

use crate::{
    director::lingo::datum::TimeoutRef,
    player::{symbols::symbol::Symbol, ScriptError},
};

use super::DatumRef;

pub struct TimeoutManager {
    pub timeouts: HashMap<TimeoutRef, Timeout>,
    next_incarnation: u64,
}

/// Incarnations cross the wasm/JavaScript ABI as a Number, so keep them below
/// JavaScript's exact-integer limit even though Rust stores them as u64.
pub const MAX_JS_SAFE_TIMEOUT_INCARNATION: u64 = 9_007_199_254_740_991;

pub struct TimeoutReplacement {
    pub old: Option<Timeout>,
    pub new: Timeout,
}

#[derive(Clone)]

pub struct Timeout {
    pub name: TimeoutRef,
    pub period: u32,
    pub handler: Symbol,
    pub target_ref: DatumRef,
    pub is_scheduled: bool,
    pub incarnation: u64,
    /// Wall-clock timestamp (ms) when this timeout should next fire.
    pub next_fire_ms: f64,
}

impl TimeoutManager {
    pub fn new() -> TimeoutManager {
        TimeoutManager {
            timeouts: HashMap::new(),
            next_incarnation: 1,
        }
    }

    fn allocate_incarnation(&mut self) -> Result<u64, ScriptError> {
        if self.next_incarnation == 0 || self.next_incarnation > MAX_JS_SAFE_TIMEOUT_INCARNATION {
            return Err(ScriptError::new("timeout incarnation exhausted".to_owned()));
        }
        let incarnation = self.next_incarnation;
        self.next_incarnation = incarnation
            .checked_add(1)
            .ok_or_else(|| ScriptError::new("timeout incarnation exhausted".to_owned()))?;
        Ok(incarnation)
    }

    fn resolve_key(&self, timeout_name: &str) -> Option<TimeoutRef> {
        if self.timeouts.contains_key(timeout_name) {
            return Some(timeout_name.to_owned());
        }
        // HashMap iteration is unordered; choose a stable key for ambiguous
        // case-insensitive fallback while preserving exact-match precedence.
        self.timeouts
            .keys()
            .filter(|key| key.eq_ignore_ascii_case(timeout_name))
            .min()
            .cloned()
    }

    pub fn get_timeout_exact(&self, timeout_name: &str) -> Option<&Timeout> {
        self.timeouts.get(timeout_name)
    }

    /// Atomically replace a timeout. Incarnation allocation occurs before any
    /// old entry is removed, so exhaustion preserves the old timer exactly.
    /// The caller turns the returned transition into ordered Clear/Schedule
    /// host actions after the player borrow has ended.
    pub fn replace_timeout(
        &mut self,
        mut timeout: Timeout,
        now_ms: f64,
    ) -> Result<TimeoutReplacement, ScriptError> {
        let incarnation = self.allocate_incarnation()?;
        // Creation/replacement is exact-keyed so `Timer` and `timer` remain
        // independent. Case-insensitive resolution is reserved for legacy
        // lookup/cancel paths below.
        let old = self.timeouts.remove(&timeout.name);
        timeout.incarnation = incarnation;
        timeout.is_scheduled = timeout.period > 0;
        timeout.next_fire_ms = if timeout.is_scheduled {
            now_ms + timeout.period as f64
        } else {
            0.0
        };

        self.timeouts.insert(timeout.name.clone(), timeout.clone());
        Ok(TimeoutReplacement { old, new: timeout })
    }

    #[allow(dead_code)]
    pub fn forget_timeout(&mut self, timeout_name: &str) -> Option<Timeout> {
        // Exact match wins. This is the common case and matches Habbo's
        // expectation that case-distinct keys stay distinct.
        if let Some(timeout) = self.timeouts.remove(timeout_name) {
            return Some(timeout);
        }
        // Fallback: case-insensitive scan. CS's cdtimer creates
        // `timeout("cdplayer").new(...)` and cancels with
        // `timeout("CDplayer").forget()` (typo); without this the cancel
        // would miss, the 1-second tick keeps firing, and once
        // `pSeconds <= 0` it spams `oStudio.sendCdStop()` every second
        // through the song. Only used when no exact match exists, so it
        // doesn't accidentally cancel a same-named-different-case entry
        // that Habbo legitimately keeps live.
        let key_to_remove = self.resolve_key(timeout_name);
        key_to_remove.and_then(|key| self.timeouts.remove(&key))
    }

    #[allow(dead_code)]
    pub fn get_timeout(&self, timeout_name: &str) -> Option<&Timeout> {
        self.resolve_key(timeout_name)
            .and_then(|key| self.timeouts.get(&key))
    }

    pub fn get_timeout_mut(&mut self, timeout_name: &str) -> Option<&mut Timeout> {
        // Resolve the actual key first (exact, then case-insensitive
        // fallback), then re-borrow mutably. Avoids the borrow-checker
        // issue with returning a &mut while also holding the iterator.
        let key = self.resolve_key(timeout_name);
        key.and_then(move |k| self.timeouts.get_mut(&k))
    }

    pub fn set_period(
        &mut self,
        timeout_name: &str,
        period: u32,
        now_ms: f64,
    ) -> Result<Option<TimeoutReplacement>, ScriptError> {
        let Some(key) = self.resolve_key(timeout_name) else {
            return Ok(None);
        };
        let incarnation = self.allocate_incarnation()?;
        let timeout = self
            .timeouts
            .get_mut(&key)
            .expect("resolved timeout key must remain present");
        let old = timeout.clone();
        timeout.period = period;
        timeout.incarnation = incarnation;
        timeout.is_scheduled = period > 0;
        timeout.next_fire_ms = if timeout.is_scheduled {
            now_ms + period as f64
        } else {
            0.0
        };
        Ok(Some(TimeoutReplacement {
            old: Some(old),
            new: timeout.clone(),
        }))
    }

    pub fn clear(&mut self) -> Vec<Timeout> {
        self.timeouts.drain().map(|(_, timeout)| timeout).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timeout(name: &str, period: u32) -> Timeout {
        Timeout {
            name: name.to_owned(),
            period,
            handler: Symbol::builtin(crate::player::symbols::builtin::BuiltInSymbol::Timeout),
            target_ref: DatumRef::Void,
            is_scheduled: false,
            incarnation: 0,
            next_fire_ms: 0.0,
        }
    }

    #[test]
    fn replacement_is_exact_first_and_period_zero_is_dormant() {
        let mut manager = TimeoutManager::new();
        let first = manager
            .replace_timeout(timeout("Timer", 10), 100.0)
            .unwrap();
        assert!(first.old.is_none());
        assert!(first.new.is_scheduled);
        let second = manager.replace_timeout(timeout("timer", 0), 200.0).unwrap();
        assert!(second.old.is_none());
        assert!(!second.new.is_scheduled);
        assert_eq!(second.new.next_fire_ms, 0.0);
        assert_eq!(manager.timeouts.len(), 2);
        assert!(manager.get_timeout_exact("Timer").is_some());
        assert_eq!(
            manager.get_timeout(&"TIMER".to_owned()).unwrap().name,
            "Timer"
        );
    }

    #[test]
    fn incarnation_allocation_failure_preserves_old_entry() {
        let mut manager = TimeoutManager::new();
        manager.replace_timeout(timeout("stable", 10), 0.0).unwrap();
        manager.next_incarnation = MAX_JS_SAFE_TIMEOUT_INCARNATION + 1;
        let result = manager.replace_timeout(timeout("stable", 20), 0.0);
        assert!(result.is_err());
        let current = manager.get_timeout_exact("stable").unwrap();
        assert_eq!(current.period, 10);
        assert_eq!(current.incarnation, 1);
    }

    #[test]
    fn maximum_safe_incarnation_is_valid_and_missing_period_does_not_consume_one() {
        let mut manager = TimeoutManager::new();
        manager.next_incarnation = MAX_JS_SAFE_TIMEOUT_INCARNATION;
        let maxed = manager.replace_timeout(timeout("maxed", 1), 0.0).unwrap();
        assert_eq!(maxed.new.incarnation, MAX_JS_SAFE_TIMEOUT_INCARNATION);
        let next = manager.next_incarnation;
        assert!(
            manager
                .set_period(&"missing".to_owned(), 1, 0.0)
                .unwrap()
                .is_none()
        );
        assert_eq!(manager.next_incarnation, next);
    }

    #[test]
    fn period_mutation_is_incarnation_qualified() {
        let mut manager = TimeoutManager::new();
        let initial = manager
            .replace_timeout(timeout("period", 10), 50.0)
            .unwrap();
        let changed = manager
            .set_period(&"period".to_owned(), 0, 60.0)
            .unwrap()
            .unwrap();
        assert_eq!(
            changed.old.as_ref().unwrap().incarnation,
            initial.new.incarnation
        );
        assert!(!changed.new.is_scheduled);
        assert!(changed.new.incarnation > initial.new.incarnation);
    }
}
