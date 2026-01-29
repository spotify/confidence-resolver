//! A concurrent bounded set that samples up to N unique items using random eviction.

use papaya::HashSet;
use std::fmt::{self, Debug};
use std::hash::Hash;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::Arc;

/// A bounded set that stores up to `N` unique items.
///
/// When full, inserting a new item evicts a randomly selected existing item.
/// Items are deduplicated by value using Hash + Eq.
///
/// # Implementation
///
/// Uses a concurrent HashSet of `Arc<T>` for O(1) membership checks and a
/// fixed-size array of raw pointers for O(1) random eviction. Each item is
/// wrapped in an Arc with refcount = 2: one reference in the set, one
/// "leaked" as a raw pointer in a slot.
///
/// This ensures memory safety under concurrent access: even if one thread
/// is probing the set while another evicts, the Arc's refcount prevents
/// use-after-free.
pub struct BoundedSet<T, const N: usize> {
    /// Set of Arc<T> for deduplication (Arc delegates Hash/Eq to T)
    set: HashSet<Arc<T>>,
    /// Fixed-size array of raw pointers for eviction tracking.
    /// Each non-null pointer was obtained from Arc::into_raw and represents
    /// one reference count that we "own".
    slots: Box<[AtomicPtr<T>; N]>,
    /// Counter for tracking inserts. Used for sequential slot filling initially,
    /// switches to random eviction once counter >= N.
    counter: AtomicUsize,
}

impl<T: Hash + Eq, const N: usize> Debug for BoundedSet<T, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoundedSet")
            .field("len", &self.set.pin().len())
            .field("capacity", &N)
            .finish()
    }
}

impl<T: Hash + Eq, const N: usize> Default for BoundedSet<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Hash + Eq, const N: usize> BoundedSet<T, N> {
    /// Creates a new empty BoundedSet.
    pub fn new() -> Self {
        // Initialize all slots to null
        let slots: Box<[AtomicPtr<T>; N]> =
            Box::new(std::array::from_fn(|_| AtomicPtr::new(ptr::null_mut())));

        BoundedSet {
            set: HashSet::with_capacity(N),
            slots,
            counter: AtomicUsize::new(0),
        }
    }

    /// Inserts an item into the set.
    ///
    /// If the item already exists (by value), this is a no-op.
    /// If the set is at capacity, a random existing item is evicted.
    pub fn insert(&self, item: T) {
        // Wrap in Arc, clone for set (refcount = 2)
        let arc = Arc::new(item);
        let arc_for_set = arc.clone();

        // "Leak" one Arc as a raw pointer for the slot (refcount stays 2)
        let ptr = Arc::into_raw(arc) as *mut T;

        let guard = self.set.pin();

        // Try to insert into set
        if !guard.insert(arc_for_set) {
            // Duplicate - reclaim our leaked Arc and drop it
            // SAFETY: ptr was just created from Arc::into_raw and never stored
            unsafe { drop(Arc::from_raw(ptr)) };
            return;
        }

        // Successfully inserted a new item
        // Pick a slot: sequential during fill phase, random after full
        if N == 0 {
            // N == 0: can't store anything, just clean up
            // Remove from set and reclaim our Arc
            // SAFETY: ptr came from Arc::into_raw above
            let arc = unsafe { Arc::from_raw(ptr) };
            guard.remove(&arc);
            return;
        }

        let slot_idx = {
            let count = self.counter.fetch_add(1, Ordering::Relaxed);
            if count < N {
                // Filling phase - sequential, no eviction possible
                count
            } else {
                // Full - random eviction
                fastrand::usize(..N)
            }
        };

        // Swap our pointer into the slot
        // SAFETY: slot_idx is always < N (from count % N or fastrand::usize(..N))
        let Some(slot) = self.slots.get(slot_idx) else {
            return;
        };
        let old_ptr = slot.swap(ptr, Ordering::AcqRel);

        // If we evicted something, remove it from the set and drop both Arcs
        if !old_ptr.is_null() {
            // SAFETY: old_ptr came from a previous Arc::into_raw in this structure
            let old_arc = unsafe { Arc::from_raw(old_ptr) };
            // Remove from set (set drops its Arc, refcount → 1)
            guard.remove(&old_arc);
            // old_arc drops here (refcount → 0, memory freed)
        }
    }

    /// Returns the number of items currently in the set.
    pub fn len(&self) -> usize {
        let count = self.counter.load(Ordering::Acquire);
        count.min(N)
    }

    /// Returns an iterator over references to items in the set.
    ///
    /// Iterates over the slots array, yielding &T for each non-null entry.
    /// Only iterates up to min(counter, N) slots for efficiency.
    pub fn iter(&self) -> BoundedSetIter<'_, T> {
        let len = self.len();
        // len is always <= N (from min(counter, N)), so this never fails
        let slots = self.slots.get(..len).unwrap_or(&[]);
        BoundedSetIter { slots, index: 0 }
    }
}

/// Iterator over references to items in a BoundedSet.
pub struct BoundedSetIter<'a, T> {
    slots: &'a [AtomicPtr<T>],
    index: usize,
}

impl<'a, T> Iterator for BoundedSetIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(slot) = self.slots.get(self.index) {
            let ptr = slot.load(Ordering::Acquire);
            self.index = self.index.saturating_add(1);
            // Slot may be null during concurrent fill: counter is incremented
            // before the pointer is stored, so we might see an in-range but
            // not-yet-filled slot.
            if !ptr.is_null() {
                // SAFETY: ptr came from Arc::into_raw and the Arc is still alive
                // (held by the set). We only yield a shared reference.
                return Some(unsafe { &*ptr });
            }
        }
        None
    }
}

impl<T, const N: usize> Drop for BoundedSet<T, N> {
    fn drop(&mut self) {
        // Reclaim all Arc references we hold via raw pointers in slots
        let count = self.counter.load(Ordering::Acquire);
        let limit = count.min(N);
        // limit is always <= N, so this slice is always valid
        let Some(slots) = self.slots.get(..limit) else {
            return;
        };
        for slot in slots {
            let ptr = slot.load(Ordering::Acquire);
            if !ptr.is_null() {
                // SAFETY: Each non-null pointer was created by Arc::into_raw
                // and represents one reference we own
                unsafe { drop(Arc::from_raw(ptr)) };
            }
        }
        // The HashSet will drop its Arc references automatically
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_dedup() {
        let set: BoundedSet<String, 10> = BoundedSet::new();

        set.insert("hello".to_string());
        set.insert("world".to_string());
        set.insert("hello".to_string()); // duplicate

        assert_eq!(set.len(), 2);
    }

    #[test]
    fn eviction_when_full() {
        let set: BoundedSet<i32, 3> = BoundedSet::new();

        // Insert more than capacity
        for i in 0..10 {
            set.insert(i);
        }

        // Should have at most N items
        assert!(set.len() <= 3);
    }

    #[test]
    fn iter_yields_all() {
        let set: BoundedSet<i32, 10> = BoundedSet::new();

        set.insert(1);
        set.insert(2);
        set.insert(3);

        // With hybrid approach, first N inserts use sequential slots
        // so no eviction happens until we exceed capacity
        let sum: i32 = set.iter().sum();
        assert_eq!(sum, 6);
    }

    #[test]
    fn empty_set() {
        let set: BoundedSet<String, 5> = BoundedSet::new();

        assert_eq!(set.len(), 0);
    }

    #[test]
    fn concurrent_stress_test() {
        use std::sync::Arc as StdArc;
        use std::thread;

        const NUM_THREADS: usize = 8;
        const INSERTS_PER_THREAD: usize = 10_000;
        const CAPACITY: usize = 50;

        let set: StdArc<BoundedSet<String, CAPACITY>> = StdArc::new(BoundedSet::new());

        let handles: Vec<_> = (0..NUM_THREADS)
            .map(|thread_id| {
                let set = StdArc::clone(&set);
                thread::spawn(move || {
                    for i in 0..INSERTS_PER_THREAD {
                        // Mix of unique and duplicate values
                        let value = format!("thread{}-item{}", thread_id, i % 100);
                        set.insert(value);
                    }
                })
            })
            .collect();

        // Wait for all threads to complete
        for handle in handles {
            handle.join().expect("thread panicked");
        }

        // Invariants that must hold:
        // 1. Length is at most CAPACITY
        let len = set.len();
        assert!(
            len <= CAPACITY,
            "len {} exceeded capacity {}",
            len,
            CAPACITY
        );

        // 2. Iterate - should not crash
        let items: Vec<_> = set.iter().collect();
        assert_eq!(items.len(), len, "iteration count mismatch");

        // 3. All items in the set are valid (not corrupted)
        for s in items {
            assert!(s.starts_with("thread"), "corrupted string: {}", s);
        }

        println!("Stress test passed: {} items in set", len);
    }

    #[test]
    fn concurrent_dedup_stress_test() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc as StdArc;
        use std::thread;

        const NUM_THREADS: usize = 8;
        const ITERATIONS: usize = 1_000;
        const CAPACITY: usize = 20;

        // All threads insert the same values - testing dedup under contention
        let set: StdArc<BoundedSet<i32, CAPACITY>> = StdArc::new(BoundedSet::new());
        let total_inserts = StdArc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..NUM_THREADS)
            .map(|_| {
                let set = StdArc::clone(&set);
                let total_inserts = StdArc::clone(&total_inserts);
                thread::spawn(move || {
                    for i in 0..ITERATIONS {
                        // All threads insert same values 0..10
                        set.insert(i as i32 % 10);
                        total_inserts.fetch_add(1, Ordering::Relaxed);
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("thread panicked");
        }

        let len = set.len();
        let total = total_inserts.load(Ordering::Relaxed);

        // With 8 threads * 1000 iterations inserting values 0-9,
        // we should have at most 10 unique values (and at most CAPACITY)
        assert!(len <= 10, "dedup failed: {} unique values", len);
        assert!(len <= CAPACITY);

        println!(
            "Dedup stress test passed: {} inserts, {} unique values",
            total, len
        );
    }

    #[test]
    fn drop_under_contention() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc as StdArc;
        use std::thread;

        const NUM_THREADS: usize = 4;
        const CAPACITY: usize = 10;

        // Test that drop works correctly even when threads were recently active
        for _ in 0..100 {
            let set: StdArc<BoundedSet<String, CAPACITY>> = StdArc::new(BoundedSet::new());
            let done = StdArc::new(AtomicBool::new(false));

            let handles: Vec<_> = (0..NUM_THREADS)
                .map(|thread_id| {
                    let set = StdArc::clone(&set);
                    let done = StdArc::clone(&done);
                    thread::spawn(move || {
                        let mut i = 0;
                        while !done.load(Ordering::Relaxed) {
                            set.insert(format!("t{}-{}", thread_id, i));
                            i += 1;
                        }
                    })
                })
                .collect();

            // Let threads run briefly
            thread::sleep(std::time::Duration::from_millis(1));
            done.store(true, Ordering::Relaxed);

            for handle in handles {
                handle.join().expect("thread panicked");
            }

            // Drop the set - this should free all memory correctly
            drop(set);
        }
        // If we get here without crashing/leaking, the test passed
    }
}
