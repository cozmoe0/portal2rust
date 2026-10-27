//! Hash table with the original interpreter's layout, so iteration order
//! matches: a power-of-two node array, chained collisions that borrow free
//! nodes from the end of the array, colliding nodes moved out of a new key's
//! main position, and the same growth and shrink rules.

use crate::value::Value;

const MIN_POWER2: usize = 4;

#[derive(Clone, Default)]
struct Node {
    key: Value,
    val: Value,
    next: Option<usize>,
}

#[derive(Clone)]
pub struct SqTable {
    nodes: Vec<Node>,
    first_free: usize,
    used: usize,
}

impl Default for SqTable {
    fn default() -> Self {
        Self::with_capacity(0)
    }
}

fn hash_key(key: &Value) -> u32 {
    match key {
        Value::Str(s) => s.hash(),
        Value::Float(f) => *f as i32 as u32,
        Value::Int(i) => *i as u32,
        Value::Bool(b) => *b as u32,
        other => other.obj().map(|o| o.index).unwrap_or(0),
    }
}

impl SqTable {
    pub fn with_capacity(n: usize) -> Self {
        let mut size = MIN_POWER2;
        while n > size {
            size <<= 1;
        }
        let mut t = Self { nodes: Vec::new(), first_free: 0, used: 0 };
        t.alloc_nodes(size);
        t
    }

    fn alloc_nodes(&mut self, size: usize) {
        self.nodes = vec![Node::default(); size];
        self.first_free = size - 1;
    }

    pub fn len(&self) -> usize {
        self.used
    }

    pub fn is_empty(&self) -> bool {
        self.used == 0
    }

    fn main_position(&self, key: &Value) -> usize {
        hash_key(key) as usize & (self.nodes.len() - 1)
    }

    fn find(&self, key: &Value) -> Option<usize> {
        let mut n = Some(self.main_position(key));
        while let Some(i) = n {
            if self.nodes[i].key.raw_eq(key) && !self.nodes[i].key.is_null() {
                return Some(i);
            }
            n = self.nodes[i].next;
        }
        None
    }

    pub fn get(&self, key: &Value) -> Option<&Value> {
        self.find(key).map(|i| &self.nodes[i].val)
    }

    pub fn contains(&self, key: &Value) -> bool {
        self.find(key).is_some()
    }

    /// Replaces the value of an existing key; false when the key is absent.
    pub fn set(&mut self, key: &Value, val: Value) -> bool {
        match self.find(key) {
            Some(i) => {
                self.nodes[i].val = val;
                true
            }
            None => false,
        }
    }

    /// Inserts or replaces. Returns true when the key was new.
    pub fn new_slot(&mut self, key: Value, val: Value) -> bool {
        debug_assert!(!key.is_null());
        if let Some(i) = self.find(&key) {
            self.nodes[i].val = val;
            return false;
        }
        let mut mp = self.main_position(&key);
        if !self.nodes[mp].key.is_null() {
            let n = self.first_free;
            let mph = self.main_position(&self.nodes[mp].key.clone());
            if mp > n && mph != mp {
                // Move the colliding node (not in its main position) to the
                // free node and relink its chain.
                let mut othern = mph;
                while self.nodes[othern].next != Some(mp) {
                    othern = self.nodes[othern].next.expect("chain reaches the colliding node");
                }
                self.nodes[othern].next = Some(n);
                self.nodes[n] = std::mem::take(&mut self.nodes[mp]);
            } else {
                // The new key goes into the free node, chained after mp.
                self.nodes[n].next = self.nodes[mp].next;
                self.nodes[mp].next = Some(n);
                mp = n;
            }
        }
        self.nodes[mp].key = key.clone();
        loop {
            let ff = &self.nodes[self.first_free];
            if ff.key.is_null() && ff.next.is_none() {
                self.nodes[mp].val = val;
                self.used += 1;
                return true;
            }
            if self.first_free == 0 {
                break;
            }
            self.first_free -= 1;
        }
        // No free node left: grow (the new key is reinserted with the rest,
        // in node order, as the original does), then store the value.
        self.rehash(true);
        if let Some(i) = self.find(&key) {
            self.nodes[i].val = val;
        }
        true
    }

    pub fn remove(&mut self, key: &Value) -> Option<Value> {
        let i = self.find(key)?;
        let old = std::mem::take(&mut self.nodes[i].val);
        self.nodes[i].key = Value::Null;
        self.used -= 1;
        self.rehash(false);
        Some(old)
    }

    fn rehash(&mut self, force: bool) {
        let old_size = self.nodes.len().max(4);
        let used = self.used;
        let new_size = if used >= old_size - old_size / 4 {
            old_size * 2
        } else if used <= old_size / 4 && old_size > MIN_POWER2 {
            old_size / 2
        } else if force {
            old_size
        } else {
            return;
        };
        let old = std::mem::take(&mut self.nodes);
        self.alloc_nodes(new_size);
        self.used = 0;
        for n in old {
            if !n.key.is_null() {
                self.new_slot(n.key, n.val);
            }
        }
        let _ = used;
    }

    pub fn clear(&mut self) {
        let size = self.nodes.len();
        self.alloc_nodes(size);
        self.used = 0;
        self.rehash(false);
    }

    /// Iteration cursor: returns the next live entry at or after `pos` and
    /// the cursor for the following call.
    pub fn next(&self, pos: usize) -> Option<(usize, &Value, &Value)> {
        (pos..self.nodes.len()).find(|&i| !self.nodes[i].key.is_null()).map(|i| (i + 1, &self.nodes[i].key, &self.nodes[i].val))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Value, &Value)> {
        self.nodes.iter().filter(|n| !n.key.is_null()).map(|n| (&n.key, &n.val))
    }

    /// Copy with the same capacity, filled in iteration order.
    pub fn duplicate(&self) -> Self {
        let mut t = Self::with_capacity(self.nodes.len());
        for (k, v) in self.iter() {
            t.new_slot(k.clone(), v.clone());
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_lookup_remove() {
        let mut t = SqTable::default();
        for i in 0..100 {
            assert!(t.new_slot(Value::Int(i), Value::Int(i * 2)));
        }
        assert_eq!(t.len(), 100);
        for i in 0..100 {
            assert!(matches!(t.get(&Value::Int(i)), Some(Value::Int(v)) if *v == i * 2));
        }
        for i in 0..50 {
            assert!(t.remove(&Value::Int(i)).is_some());
        }
        assert_eq!(t.len(), 50);
        assert!(t.get(&Value::Int(10)).is_none());
        assert!(t.get(&Value::Int(60)).is_some());
        assert_eq!(t.iter().count(), 50);
    }

    #[test]
    fn small_integer_keys_iterate_in_slot_order() {
        let mut t = SqTable::default();
        for i in [3, 1, 2, 0] {
            t.new_slot(Value::Int(i), Value::Null);
        }
        let keys: Vec<i32> = t.iter().map(|(k, _)| k.as_i32().unwrap()).collect();
        assert_eq!(keys, vec![0, 1, 2, 3]);
    }
}
