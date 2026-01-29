//! A concurrent bounded set that samples up to N unique items using random eviction.

use std::fmt::{self, Debug};
use std::hash::Hash;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::Arc;
use papaya::HashSet;
use std::ptr;

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
    /// Wrapped in Option so into_iter can take ownership without allocation.
    slots: Option<Box<[AtomicPtr<T>; N]>>,
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
            set: HashSet::new(),
            slots: Some(slots),
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
        // slots is always Some during normal operation (only None after into_iter)
        let slots = self.slots.as_ref().expect("slots taken by into_iter");
        let slot = &slots[slot_idx];
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
        self.set.pin().len()
    }

    /// Returns true if the set is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns an iterator over references to items in the set.
    /// 
    /// Iterates over the slots array, yielding &T for each non-null entry.
    /// Only iterates up to min(counter, N) slots for efficiency.
    pub fn iter(&self) -> BoundedSetIter<'_, T> {
        let slots = self.slots.as_ref().expect("slots taken by into_iter");
        let count = self.counter.load(Ordering::Acquire);
        let len = count.min(N);
        BoundedSetIter {
            slots: &slots[..len],
            index: 0,
        }
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
        while self.index < self.slots.len() {
            let ptr = self.slots[self.index].load(Ordering::Acquire);
            self.index += 1;
            if !ptr.is_null() {
                // SAFETY: ptr came from Arc::into_raw and the Arc is still alive
                // (held by the set). We only yield a shared reference.
                return Some(unsafe { &*ptr });
            }
        }
        None
    }
}

impl<T: Hash + Eq, const N: usize> IntoIterator for BoundedSet<T, N> {
    type Item = T;
    type IntoIter = BoundedSetIntoIter<T, N>;

    /// Consumes the set and returns an iterator over owned values.
    /// 
    /// This is zero-allocation: we take the slots, then let self drop
    /// (which drops the set, decrementing refcounts to 1). The iterator
    /// then owns the slots and can unwrap each Arc to get owned T.
    fn into_iter(mut self) -> Self::IntoIter {
        // Take the slots. Drop will find None and skip cleanup.
        let slots = self.slots.take().expect("slots already taken");
        let count = self.counter.load(Ordering::Acquire);
        let limit = count.min(N);
        
        // self drops here:
        // - self.set drops → Arc<T> refcounts go from 2 to 1
        // - self.slots is None, Drop does nothing
        
        BoundedSetIntoIter {
            slots,
            limit,
            index: 0,
        }
    }
}

/// An iterator that yields owned values from a consumed `BoundedSet`.
pub struct BoundedSetIntoIter<T, const N: usize> {
    slots: Box<[AtomicPtr<T>; N]>,
    limit: usize,
    index: usize,
}

impl<T, const N: usize> Iterator for BoundedSetIntoIter<T, N> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        while self.index < self.limit {
            let ptr = self.slots[self.index].load(Ordering::Acquire);
            self.index += 1;
            
            if !ptr.is_null() {
                // SAFETY: ptr came from Arc::into_raw, and we dropped the set
                // so refcount is now 1 - we're the sole owner
                let arc = unsafe { Arc::from_raw(ptr) };
                // Unwrap the Arc - refcount is 1, so this always succeeds
                return Some(Arc::into_inner(arc).expect("refcount should be 1"));
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        // We don't know exactly how many non-null slots remain
        (0, Some(self.limit.saturating_sub(self.index)))
    }
}

impl<T, const N: usize> Drop for BoundedSetIntoIter<T, N> {
    fn drop(&mut self) {
        // Clean up any remaining slots we haven't iterated
        while self.index < self.limit {
            let ptr = self.slots[self.index].load(Ordering::Acquire);
            self.index += 1;
            if !ptr.is_null() {
                // SAFETY: ptr came from Arc::into_raw
                unsafe { drop(Arc::from_raw(ptr)) };
            }
        }
    }
}

impl<T, const N: usize> Drop for BoundedSet<T, N> {
    fn drop(&mut self) {
        // Reclaim all Arc references we hold via raw pointers in slots
        // (slots is None if into_iter was called)
        if let Some(ref slots) = self.slots {
            let count = self.counter.load(Ordering::Acquire);
            let limit = count.min(N);
            for slot in &slots[..limit] {
                let ptr = slot.load(Ordering::Acquire);
                if !ptr.is_null() {
                    // SAFETY: Each non-null pointer was created by Arc::into_raw
                    // and represents one reference we own
                    unsafe { drop(Arc::from_raw(ptr)) };
                }
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
    fn into_iter_yields_all() {
        let set: BoundedSet<i32, 10> = BoundedSet::new();
        
        set.insert(1);
        set.insert(2);
        set.insert(3);
        
        // With hybrid approach, first N inserts use sequential slots
        // so no eviction happens until we exceed capacity
        let sum: i32 = set.into_iter().sum();
        assert_eq!(sum, 6);
    }

    #[test]
    fn empty_set() {
        let set: BoundedSet<String, 5> = BoundedSet::new();
        
        assert!(set.is_empty());
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
        assert!(len <= CAPACITY, "len {} exceeded capacity {}", len, CAPACITY);

        // Get exclusive access now that all threads are done
        let set = StdArc::try_unwrap(set).expect("Arc should have single owner");

        // 2. Consume and iterate - should not crash
        let items: Vec<_> = set.into_iter().collect();
        assert_eq!(items.len(), len, "iteration count mismatch");

        // 3. All items in the set are valid (not corrupted)
        for s in &items {
            assert!(s.starts_with("thread"), "corrupted string: {}", s);
        }

        println!("Stress test passed: {} items in set", len);
    }

    #[test]
    fn concurrent_dedup_stress_test() {
        use std::sync::Arc as StdArc;
        use std::thread;
        use std::sync::atomic::{AtomicUsize, Ordering};

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
        use std::sync::Arc as StdArc;
        use std::thread;
        use std::sync::atomic::{AtomicBool, Ordering};

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
