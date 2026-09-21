use std::{
    cmp::Ordering,
    collections::HashMap,
};

use crate::{
    director::lingo::datum::TimeoutRef,
    player::{symbols::symbol::Symbol, ScriptError},
};

use super::DatumRef;

pub struct TimeoutManager {
    pub timeouts: HashMap<TimeoutRef, Timeout>,
    next_incarnation: u64,
    next_registration_sequence: u64,
}

/// The exact logical time used by the native caller-driven pump.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct NativeTime {
    numerator: u128,
    denominator: u128,
}

pub(crate) const MAX_EXACT_NATIVE_MS: u64 = (1u64 << 53) - 1;
pub(crate) const MAX_EXACT_NATIVE_US: u64 = MAX_EXACT_NATIVE_MS.saturating_mul(1_000);

impl NativeTime {
    pub(crate) fn from_ms(milliseconds: u64) -> Result<Self, ScriptError> {
        if milliseconds > MAX_EXACT_NATIVE_MS {
            return Err(ScriptError::new(
                "native simulation time exceeds exact supported range".to_owned(),
            ));
        }
        Ok(Self { numerator: milliseconds as u128, denominator: 1 })
    }

    pub(crate) fn from_micros(microseconds: u64) -> Result<Self, ScriptError> {
        if microseconds > MAX_EXACT_NATIVE_US {
            return Err(ScriptError::new(
                "native simulation time exceeds exact supported range".to_owned(),
            ));
        }
        Ok(Self { numerator: microseconds as u128, denominator: 1_000 })
    }

    pub(crate) fn zero() -> Self {
        Self { numerator: 0, denominator: 1 }
    }

    pub(crate) fn add_integer_ms(self, milliseconds: u32) -> Result<Self, ScriptError> {
        self.add_fraction(milliseconds as u128, 1)
    }

    pub(crate) fn add_fraction(self, numerator: u128, denominator: u128) -> Result<Self, ScriptError> {
        if denominator == 0 {
            return Err(ScriptError::new("native time has a zero denominator".to_owned()));
        }
        let common_denominator = gcd(self.denominator, denominator);
        let left_scale = denominator / common_denominator;
        let right_scale = self.denominator / common_denominator;
        let left = self
            .numerator
            .checked_mul(left_scale)
            .ok_or_else(|| ScriptError::new("native time deadline overflow".to_owned()))?;
        let right = numerator
            .checked_mul(right_scale)
            .ok_or_else(|| ScriptError::new("native time deadline overflow".to_owned()))?;
        let denominator = self
            .denominator
            .checked_mul(left_scale)
            .ok_or_else(|| ScriptError::new("native time deadline overflow".to_owned()))?;
        let numerator = left
            .checked_add(right)
            .ok_or_else(|| ScriptError::new("native time deadline overflow".to_owned()))?;
        let common = gcd(numerator, denominator);
        Ok(Self { numerator: numerator / common, denominator: denominator / common })
    }

    pub(crate) fn cmp(self, other: Self) -> Result<Ordering, ScriptError> {
        let left = self
            .numerator
            .checked_mul(other.denominator)
            .ok_or_else(|| ScriptError::new("native time comparison overflow".to_owned()))?;
        let right = other
            .numerator
            .checked_mul(self.denominator)
            .ok_or_else(|| ScriptError::new("native time comparison overflow".to_owned()))?;
        Ok(left.cmp(&right))
    }

    pub(crate) fn to_f64(self) -> Result<f64, ScriptError> {
        let value = self.numerator as f64 / self.denominator as f64;
        if value.is_finite() {
            Ok(value)
        } else {
            Err(ScriptError::new("native time cannot be represented as f64".to_owned()))
        }
    }

    pub(crate) fn to_micros(self) -> Result<u64, ScriptError> {
        let micros = self.to_f64()? * 1_000.0;
        if !micros.is_finite() || micros < 0.0 || micros > u64::MAX as f64 {
            return Err(ScriptError::new(
                "native time cannot be represented in microseconds".to_owned(),
            ));
        }
        Ok(micros.round() as u64)
    }

    /// Return whole milliseconds using the native clock's exact rational value.
    /// Native Lingo exposes `milliSeconds` as an integer, so fractional values
    /// are truncated without passing through floating point.
    pub(crate) fn to_milliseconds_floor(self) -> Result<u64, ScriptError> {
        let milliseconds = self.numerator / self.denominator;
        u64::try_from(milliseconds)
            .map_err(|_| ScriptError::new("native milliseconds exceed supported range".to_owned()))
    }

    /// Return whole Director ticks (60 per second) from the exact native time.
    pub(crate) fn to_ticks_floor(self) -> Result<i32, ScriptError> {
        let scaled_numerator = self
            .numerator
            .checked_mul(60)
            .ok_or_else(|| ScriptError::new("native tick conversion overflow".to_owned()))?;
        let scaled_denominator = self
            .denominator
            .checked_mul(1_000)
            .ok_or_else(|| ScriptError::new("native tick conversion overflow".to_owned()))?;
        let ticks = scaled_numerator / scaled_denominator;
        i32::try_from(ticks)
            .map_err(|_| ScriptError::new("native ticks exceed supported range".to_owned()))
    }
}

fn gcd(mut left: u128, mut right: u128) -> u128 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left.max(1)
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
    /// Exact native deadline. Browser/legacy paths continue using next_fire_ms.
    pub(crate) native_next_fire: Option<NativeTime>,
    pub(crate) registration_sequence: u64,
}

impl TimeoutManager {
    pub fn new() -> TimeoutManager {
        TimeoutManager {
            timeouts: HashMap::new(),
            next_incarnation: 1,
            next_registration_sequence: 1,
        }
    }

    fn allocate_registration_sequence(&mut self) -> Result<u64, ScriptError> {
        let sequence = self.next_registration_sequence;
        self.next_registration_sequence = sequence
            .checked_add(1)
            .ok_or_else(|| ScriptError::new("timeout registration sequence exhausted".to_owned()))?;
        Ok(sequence)
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
        self.replace_timeout_inner(timeout, now_ms, None)
    }

    pub fn replace_timeout_at(
        &mut self,
        timeout: Timeout,
        now: NativeTime,
    ) -> Result<TimeoutReplacement, ScriptError> {
        self.replace_timeout_inner(timeout, now.to_f64()?, Some(now))
    }

    fn replace_timeout_inner(
        &mut self,
        mut timeout: Timeout,
        now_ms: f64,
        native_now: Option<NativeTime>,
    ) -> Result<TimeoutReplacement, ScriptError> {
        let incarnation = self.allocate_incarnation()?;
        let registration_sequence = self.allocate_registration_sequence()?;
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
        timeout.native_next_fire = if timeout.is_scheduled { native_now.map(|now| now.add_integer_ms(timeout.period)).transpose()? } else { None };
        timeout.registration_sequence = registration_sequence;

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
        self.set_period_inner(timeout_name, period, now_ms, None)
    }

    pub fn set_period_at(
        &mut self,
        timeout_name: &str,
        period: u32,
        now: NativeTime,
    ) -> Result<Option<TimeoutReplacement>, ScriptError> {
        self.set_period_inner(timeout_name, period, now.to_f64()?, Some(now))
    }

    fn set_period_inner(
        &mut self,
        timeout_name: &str,
        period: u32,
        now_ms: f64,
        native_now: Option<NativeTime>,
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
        timeout.native_next_fire = if timeout.is_scheduled { native_now.map(|now| now.add_integer_ms(period)).transpose()? } else { None };
        Ok(Some(TimeoutReplacement {
            old: Some(old),
            new: timeout.clone(),
        }))
    }

    pub fn clear(&mut self) -> Vec<Timeout> {
        self.timeouts.drain().map(|(_, timeout)| timeout).collect()
    }

    pub(crate) fn next_native_deadline(&self) -> Result<Option<NativeTime>, ScriptError> {
        let mut next = None;
        for timeout in self.timeouts.values() {
            let Some(deadline) = timeout.native_next_fire else { continue };
            if !timeout.is_scheduled { continue }
            if next.is_none() || deadline.cmp(next.unwrap())? == Ordering::Less {
                next = Some(deadline);
            }
        }
        Ok(next)
    }

    pub(crate) fn due_native_timeouts(&mut self, now: NativeTime) -> Result<Vec<Timeout>, ScriptError> {
        let mut due = Vec::new();
        for timeout in self.timeouts.values_mut() {
            let Some(deadline) = timeout.native_next_fire else { continue };
            if !timeout.is_scheduled || deadline.cmp(now)? == Ordering::Greater { continue }
            timeout.native_next_fire = Some(deadline.add_integer_ms(timeout.period)?);
            timeout.next_fire_ms = timeout.native_next_fire.unwrap().to_f64()?;
            due.push((deadline, timeout.clone()));
        }
        due.sort_by(|(left_deadline, left), (right_deadline, right)| {
            left_deadline
                .cmp(*right_deadline)
                .unwrap_or(Ordering::Equal)
                .then(left.registration_sequence.cmp(&right.registration_sequence))
        });
        Ok(due.into_iter().map(|(_, timeout)| timeout).collect())
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
            native_next_fire: None,
            registration_sequence: 0,
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

    #[test]
    fn native_deadlines_are_exact_and_registration_order_is_stable() {
        let zero = NativeTime::zero();
        let first_frame = zero.add_fraction(1000, 30).unwrap();
        let second_frame = first_frame.add_fraction(1000, 30).unwrap();
        assert_eq!(first_frame.cmp(NativeTime::from_ms(33).unwrap()).unwrap(), Ordering::Greater);
        assert_eq!(second_frame.cmp(NativeTime::from_ms(66).unwrap()).unwrap(), Ordering::Greater);

        let mut manager = TimeoutManager::new();
        manager.replace_timeout_at(timeout("first", 100), zero).unwrap();
        manager.replace_timeout_at(timeout("second", 100), zero).unwrap();
        let due = manager.due_native_timeouts(NativeTime::from_ms(100).unwrap()).unwrap();
        assert_eq!(due.iter().map(|timeout| timeout.name.as_str()).collect::<Vec<_>>(), ["first", "second"]);
    }

    #[test]
    fn native_microsecond_boundary_preserves_rational_frame_deadline() {
        let first_frame = NativeTime::zero().add_fraction(1000, 30).unwrap();
        assert_eq!(
            first_frame.cmp(NativeTime::from_micros(33_333).unwrap()).unwrap(),
            Ordering::Greater
        );
        assert_eq!(
            first_frame.cmp(NativeTime::from_micros(33_334).unwrap()).unwrap(),
            Ordering::Less
        );
    }

    #[test]
    fn native_clock_conversions_use_exact_floor_units() {
        let cases = [
            (0, 0, 0),
            (16_666, 16, 0),
            (16_667, 16, 1),
            (100_000, 100, 6),
            (1_000_000, 1_000, 60),
        ];
        for (microseconds, milliseconds, ticks) in cases {
            let now = NativeTime::from_micros(microseconds).unwrap();
            assert_eq!(now.to_milliseconds_floor().unwrap(), milliseconds);
            assert_eq!(now.to_ticks_floor().unwrap(), ticks);
        }
    }

    #[test]
    fn native_recurring_deadline_advances_from_due_boundary_and_replacement_fences_old_incarnation() {
        let mut manager = TimeoutManager::new();
        let initial = manager.replace_timeout_at(timeout("repeat", 100), NativeTime::zero()).unwrap();
        let due = manager.due_native_timeouts(NativeTime::from_ms(100).unwrap()).unwrap();
        assert_eq!(due[0].incarnation, initial.new.incarnation);
        assert_eq!(manager.next_native_deadline().unwrap(), Some(NativeTime::from_ms(200).unwrap()));

        let replacement = manager.replace_timeout_at(timeout("repeat", 50), NativeTime::from_ms(100).unwrap()).unwrap();
        assert!(replacement.new.incarnation > initial.new.incarnation);
        assert_eq!(manager.next_native_deadline().unwrap(), Some(NativeTime::from_ms(150).unwrap()));
        let due = manager.due_native_timeouts(NativeTime::from_ms(200).unwrap()).unwrap();
        assert_eq!(due[0].incarnation, replacement.new.incarnation);
    }
}
