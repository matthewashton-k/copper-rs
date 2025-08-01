//! CopperList is the main data structure used by Copper to communicate between tasks.
//! It is a queue that can be used to store preallocated messages between tasks in memory order.
extern crate alloc;

use bincode::{Decode, Encode};
use std::fmt;

use cu29_traits::{CopperListTuple, ErasedCuStampedData, ErasedCuStampedDataSet};
use serde_derive::Serialize;
use std::fmt::Display;
use std::iter::{Chain, Rev};
use std::slice::{Iter as SliceIter, IterMut as SliceIterMut};

const MAX_TASKS: usize = 512;

/// Not implemented yet.
/// This mask will be used to for example filter out necessary regions of a copper list between remote systems.
#[derive(Debug, Encode, Decode, PartialEq, Clone, Copy)]
pub struct CopperLiskMask {
    #[allow(dead_code)]
    mask: [u128; MAX_TASKS / 128 + 1],
}

/// Those are the possible states along the lifetime of a CopperList.
#[derive(Debug, Encode, Decode, Serialize, PartialEq, Copy, Clone)]
pub enum CopperListState {
    Free,
    Initialized,
    Processing,
    DoneProcessing,
    BeingSerialized,
}

impl Display for CopperListState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CopperListState::Free => write!(f, "Free"),
            CopperListState::Initialized => write!(f, "Initialized"),
            CopperListState::Processing => write!(f, "Processing"),
            CopperListState::DoneProcessing => write!(f, "DoneProcessing"),
            CopperListState::BeingSerialized => write!(f, "BeingSerialized"),
        }
    }
}

#[derive(Debug, Encode, Decode, Serialize)]
pub struct CopperList<P: CopperListTuple> {
    pub id: u32,
    state: CopperListState,
    pub msgs: P, // This is generated from the runtime.
}

impl<P: CopperListTuple> Default for CopperList<P> {
    fn default() -> Self {
        CopperList {
            id: 0,
            state: CopperListState::Free,
            msgs: P::default(),
        }
    }
}

impl<P: CopperListTuple> CopperList<P> {
    // This is not the usual way to create a CopperList, this is just for testing.
    pub fn new(id: u32, msgs: P) -> Self {
        CopperList {
            id,
            state: CopperListState::Initialized,
            msgs,
        }
    }

    pub fn change_state(&mut self, new_state: CopperListState) {
        self.state = new_state; // TODO: probably wise here to enforce a state machine.
    }

    pub fn get_state(&self) -> CopperListState {
        self.state
    }
}

impl<P: CopperListTuple> ErasedCuStampedDataSet for CopperList<P> {
    fn cumsgs(&self) -> Vec<&dyn ErasedCuStampedData> {
        self.msgs.cumsgs()
    }
    fn cumsgs_mut(&mut self) -> Vec<&mut dyn ErasedCuStampedData> {
        self.msgs.cumsgs_mut()
    }
}

/// This structure maintains the entire memory needed by Copper for one loop for the inter tasks communication within a process.
/// P or Payload is typically a Tuple of various types of messages that are exchanged between tasks.
/// N is the maximum number of in flight Copper List the runtime can support.
pub struct CuListsManager<P: CopperListTuple, const N: usize> {
    data: Box<[CopperList<P>; N]>,
    length: usize,
    insertion_index: usize,
    current_cl_id: u32,
}

impl<P: CopperListTuple + fmt::Debug, const N: usize> fmt::Debug for CuListsManager<P, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CuListsManager")
            .field("data", &self.data)
            .field("length", &self.length)
            .field("insertion_index", &self.insertion_index)
            // Do not include on_drop field
            .finish()
    }
}

pub type Iter<'a, T> = Chain<Rev<SliceIter<'a, T>>, Rev<SliceIter<'a, T>>>;
pub type IterMut<'a, T> = Chain<Rev<SliceIterMut<'a, T>>, Rev<SliceIterMut<'a, T>>>;
pub type AscIter<'a, T> = Chain<SliceIter<'a, T>, SliceIter<'a, T>>;
pub type AscIterMut<'a, T> = Chain<SliceIterMut<'a, T>, SliceIterMut<'a, T>>;

/// Initializes fields that cannot be zeroed after allocating a zeroed
/// [`CopperList`].
pub trait CuListZeroedInit: CopperListTuple {
    /// Fixes up a zero-initialized copper list so that all internal fields are
    /// in a valid state.
    fn init_zeroed(&mut self);
}

impl<P: CopperListTuple + CuListZeroedInit, const N: usize> Default for CuListsManager<P, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: CopperListTuple, const N: usize> CuListsManager<P, N> {
    pub fn new() -> Self
    where
        P: CuListZeroedInit,
    {
        let data = unsafe {
            let layout = std::alloc::Layout::new::<[CopperList<P>; N]>();
            let ptr = std::alloc::alloc_zeroed(layout) as *mut [CopperList<P>; N];
            if ptr.is_null() {
                std::alloc::handle_alloc_error(layout);
            }
            Box::from_raw(ptr)
        };
        let mut manager = CuListsManager {
            data,
            length: 0,
            insertion_index: 0,
            current_cl_id: 0,
        };

        for cl in manager.data.iter_mut() {
            cl.msgs.init_zeroed();
        }

        manager
    }

    /// Returns the current number of elements in the queue.
    ///
    #[inline]
    pub fn len(&self) -> usize {
        self.length
    }

    /// Returns `true` if the queue contains no elements.
    ///
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Returns `true` if the queue is full.
    ///
    #[inline]
    pub fn is_full(&self) -> bool {
        N == self.len()
    }

    /// Clears the queue.
    ///
    #[inline]
    pub fn clear(&mut self) {
        self.insertion_index = 0;
        self.length = 0;
    }

    #[inline]
    pub fn create(&mut self) -> Option<&mut CopperList<P>> {
        if self.is_full() {
            return None;
        }
        let result = &mut self.data[self.insertion_index];
        self.insertion_index = (self.insertion_index + 1) % N;
        self.length += 1;

        // We assign a unique id to each CopperList to be able to track them across their lifetime.
        result.id = self.current_cl_id;
        self.current_cl_id += 1;

        Some(result)
    }

    /// Peeks at the last element in the queue.
    #[inline]
    pub fn peek(&self) -> Option<&CopperList<P>> {
        if self.length == 0 {
            return None;
        }
        let index = if self.insertion_index == 0 {
            N - 1
        } else {
            self.insertion_index - 1
        };
        Some(&self.data[index])
    }

    #[inline]
    #[allow(dead_code)]
    fn drop_last(&mut self) {
        if self.length == 0 {
            return;
        }
        if self.insertion_index == 0 {
            self.insertion_index = N - 1;
        } else {
            self.insertion_index -= 1;
        }
        self.length -= 1;
    }

    #[inline]
    pub fn pop(&mut self) -> Option<&mut CopperList<P>> {
        if self.length == 0 {
            return None;
        }
        if self.insertion_index == 0 {
            self.insertion_index = N - 1;
        } else {
            self.insertion_index -= 1;
        }
        self.length -= 1;
        Some(&mut self.data[self.insertion_index])
    }

    /// Returns an iterator over the queue's contents.
    ///
    /// The iterator goes from the most recently pushed items to the oldest ones.
    ///
    #[inline]
    pub fn iter(&self) -> Iter<CopperList<P>> {
        let (a, b) = self.data[0..self.length].split_at(self.insertion_index);
        a.iter().rev().chain(b.iter().rev())
    }

    /// Returns a mutable iterator over the queue's contents.
    ///
    /// The iterator goes from the most recently pushed items to the oldest ones.
    ///
    #[inline]
    pub fn iter_mut(&mut self) -> IterMut<CopperList<P>> {
        let (a, b) = self.data.split_at_mut(self.insertion_index);
        a.iter_mut().rev().chain(b.iter_mut().rev())
    }

    /// Returns an ascending iterator over the queue's contents.
    ///
    /// The iterator goes from the least recently pushed items to the newest ones.
    ///
    #[inline]
    pub fn asc_iter(&self) -> AscIter<CopperList<P>> {
        let (a, b) = self.data.split_at(self.insertion_index);
        b.iter().chain(a.iter())
    }

    /// Returns a mutable ascending iterator over the queue's contents.
    ///
    /// The iterator goes from the least recently pushed items to the newest ones.
    ///
    #[inline]
    pub fn asc_iter_mut(&mut self) -> AscIterMut<CopperList<P>> {
        let (a, b) = self.data.split_at_mut(self.insertion_index);
        b.iter_mut().chain(a.iter_mut())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cu29_traits::{ErasedCuStampedData, ErasedCuStampedDataSet, MatchingTasks};
    use serde::{Serialize, Serializer};

    #[derive(Debug, Encode, Decode, PartialEq, Clone, Copy, Serialize, Default)]
    struct CuStampedDataSet(i32);

    impl ErasedCuStampedDataSet for CuStampedDataSet {
        fn cumsgs(&self) -> Vec<&dyn ErasedCuStampedData> {
            Vec::new()
        }
        fn cumsgs_mut(&mut self) -> Vec<&mut dyn ErasedCuStampedData> {
            Vec::new()
        }
    }

    impl MatchingTasks for CuStampedDataSet {
        fn get_all_task_ids() -> &'static [&'static str] {
            &[]
        }
    }

    impl CuListZeroedInit for CuStampedDataSet {
        fn init_zeroed(&mut self) {}
    }

    #[test]
    fn empty_queue() {
        let q = CuListsManager::<CuStampedDataSet, 5>::new();

        assert!(q.is_empty());
        assert!(q.iter().next().is_none());
    }

    #[test]
    fn partially_full_queue() {
        let mut q = CuListsManager::<CuStampedDataSet, 5>::new();
        q.create().unwrap().msgs.0 = 1;
        q.create().unwrap().msgs.0 = 2;
        q.create().unwrap().msgs.0 = 3;

        assert!(!q.is_empty());
        assert_eq!(q.len(), 3);

        let res: Vec<i32> = q.iter().map(|x| x.msgs.0).collect();
        assert_eq!(res, [3, 2, 1]);
    }

    #[test]
    fn full_queue() {
        let mut q = CuListsManager::<CuStampedDataSet, 5>::new();
        q.create().unwrap().msgs.0 = 1;
        q.create().unwrap().msgs.0 = 2;
        q.create().unwrap().msgs.0 = 3;
        q.create().unwrap().msgs.0 = 4;
        q.create().unwrap().msgs.0 = 5;
        assert_eq!(q.len(), 5);

        let res: Vec<_> = q.iter().map(|x| x.msgs.0).collect();
        assert_eq!(res, [5, 4, 3, 2, 1]);
    }

    #[test]
    fn over_full_queue() {
        let mut q = CuListsManager::<CuStampedDataSet, 5>::new();
        q.create().unwrap().msgs.0 = 1;
        q.create().unwrap().msgs.0 = 2;
        q.create().unwrap().msgs.0 = 3;
        q.create().unwrap().msgs.0 = 4;
        q.create().unwrap().msgs.0 = 5;
        assert!(q.create().is_none());
        assert_eq!(q.len(), 5);

        let res: Vec<_> = q.iter().map(|x| x.msgs.0).collect();
        assert_eq!(res, [5, 4, 3, 2, 1]);
    }

    #[test]
    fn clear() {
        let mut q = CuListsManager::<CuStampedDataSet, 5>::new();
        q.create().unwrap().msgs.0 = 1;
        q.create().unwrap().msgs.0 = 2;
        q.create().unwrap().msgs.0 = 3;
        q.create().unwrap().msgs.0 = 4;
        q.create().unwrap().msgs.0 = 5;
        assert!(q.create().is_none());
        assert_eq!(q.len(), 5);

        q.clear();

        assert_eq!(q.len(), 0);
        assert!(q.iter().next().is_none());

        q.create().unwrap().msgs.0 = 1;
        q.create().unwrap().msgs.0 = 2;
        q.create().unwrap().msgs.0 = 3;

        assert_eq!(q.len(), 3);

        let res: Vec<_> = q.iter().map(|x| x.msgs.0).collect();
        assert_eq!(res, [3, 2, 1]);
    }

    #[test]
    fn mutable_iterator() {
        let mut q = CuListsManager::<CuStampedDataSet, 5>::new();
        q.create().unwrap().msgs.0 = 1;
        q.create().unwrap().msgs.0 = 2;
        q.create().unwrap().msgs.0 = 3;
        q.create().unwrap().msgs.0 = 4;
        q.create().unwrap().msgs.0 = 5;

        for x in q.iter_mut() {
            x.msgs.0 *= 2;
        }

        let res: Vec<_> = q.iter().map(|x| x.msgs.0).collect();
        assert_eq!(res, [10, 8, 6, 4, 2]);
    }

    #[test]
    fn test_drop_last() {
        let mut q = CuListsManager::<CuStampedDataSet, 5>::new();
        q.create().unwrap().msgs.0 = 1;
        q.create().unwrap().msgs.0 = 2;
        q.create().unwrap().msgs.0 = 3;
        q.create().unwrap().msgs.0 = 4;
        q.create().unwrap().msgs.0 = 5;
        assert_eq!(q.len(), 5);

        q.drop_last();
        assert_eq!(q.len(), 4);

        let res: Vec<_> = q.iter().map(|x| x.msgs.0).collect();
        assert_eq!(res, [4, 3, 2, 1]);
    }

    #[test]
    fn test_pop() {
        let mut q = CuListsManager::<CuStampedDataSet, 5>::new();
        q.create().unwrap().msgs.0 = 1;
        q.create().unwrap().msgs.0 = 2;
        q.create().unwrap().msgs.0 = 3;
        q.create().unwrap().msgs.0 = 4;
        q.create().unwrap().msgs.0 = 5;
        assert_eq!(q.len(), 5);

        let last = q.pop().unwrap();
        assert_eq!(last.msgs.0, 5);
        assert_eq!(q.len(), 4);

        let res: Vec<_> = q.iter().map(|x| x.msgs.0).collect();
        assert_eq!(res, [4, 3, 2, 1]);
    }

    #[test]
    fn test_peek() {
        let mut q = CuListsManager::<CuStampedDataSet, 5>::new();
        q.create().unwrap().msgs.0 = 1;
        q.create().unwrap().msgs.0 = 2;
        q.create().unwrap().msgs.0 = 3;
        q.create().unwrap().msgs.0 = 4;
        q.create().unwrap().msgs.0 = 5;
        assert_eq!(q.len(), 5);

        let last = q.peek().unwrap();
        assert_eq!(last.msgs.0, 5);
        assert_eq!(q.len(), 5);

        let res: Vec<_> = q.iter().map(|x| x.msgs.0).collect();
        assert_eq!(res, [5, 4, 3, 2, 1]);
    }

    #[derive(Decode, Encode, Debug, PartialEq, Clone, Copy)]
    struct TestStruct {
        content: [u8; 10_000_000],
    }

    impl Default for TestStruct {
        fn default() -> Self {
            TestStruct {
                content: [0; 10_000_000],
            }
        }
    }

    impl ErasedCuStampedDataSet for TestStruct {
        fn cumsgs(&self) -> Vec<&dyn ErasedCuStampedData> {
            Vec::new()
        }
        fn cumsgs_mut(&mut self) -> Vec<&mut dyn ErasedCuStampedData> {
            Vec::new()
        }
    }

    impl Serialize for TestStruct {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serializer.serialize_i8(0)
        }
    }

    impl MatchingTasks for TestStruct {
        fn get_all_task_ids() -> &'static [&'static str] {
            &[]
        }
    }

    impl CuListZeroedInit for TestStruct {
        fn init_zeroed(&mut self) {}
    }

    #[test]
    fn be_sure_we_wont_stackoverflow_at_init() {
        let _ = CuListsManager::<TestStruct, 3>::new();
    }
}
