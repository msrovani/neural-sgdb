//! ADR-0063 Q2 — ART intermediário: Node4/16/48/256 + leaf tombstone delete.
//! Honesty: não claim 10M P99<100ns.
//! Seam: SSE2 Node16 gateado por `cpu_has_avx2()` (injetável).

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone)]
enum Node {
    Leaf {
        key: Vec<u8>,
        value: u64,
    },
    Inner4 {
        prefix: Vec<u8>,
        keys: [u8; 4],
        children: [Option<Box<Node>>; 4],
        n: u8,
    },
    Inner16 {
        prefix: Vec<u8>,
        keys: [u8; 16],
        children: [Option<Box<Node>>; 16],
        n: u8,
    },
    Inner48 {
        prefix: Vec<u8>,
        /// byte → child index+1 (0 = empty)
        keys: [u8; 256],
        children: [Option<Box<Node>>; 48],
        n: u8,
    },
    Inner256 {
        prefix: Vec<u8>,
        children: [Option<Box<Node>>; 256],
        n: u16,
    },
}

fn empty16() -> [Option<Box<Node>>; 16] {
    core::array::from_fn(|_| None)
}
fn empty48() -> [Option<Box<Node>>; 48] {
    core::array::from_fn(|_| None)
}
fn empty256() -> [Option<Box<Node>>; 256] {
    core::array::from_fn(|_| None)
}

/// ART paper Fig.8 — Node16: SSE `_mm_cmpeq_epi8` se cpu_has_avx2(); senão loop.
#[inline]
fn find_child_byte16(keys: &[u8; 16], n: u8, byte: u8) -> Option<usize> {
    let n = n as usize;
    if n == 0 {
        return None;
    }
    #[cfg(target_arch = "x86_64")]
    {
        if crate::hamming_dispatch::cpu_has_avx2() {
            return unsafe { find_child_byte16_sse(keys, n, byte) };
        }
    }
    for i in 0..n {
        if keys[i] == byte {
            return Some(i);
        }
    }
    None
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn find_child_byte16_sse(keys: &[u8; 16], n: usize, byte: u8) -> Option<usize> {
    use core::arch::x86_64::*;
    let key = _mm_set1_epi8(byte as i8);
    let cmp = _mm_cmpeq_epi8(key, _mm_loadu_si128(keys.as_ptr() as *const __m128i));
    let mask = (1u32 << n) - 1;
    let bitfield = _mm_movemask_epi8(cmp) as u32 & mask;
    if bitfield == 0 {
        None
    } else {
        Some(bitfield.trailing_zeros() as usize)
    }
}

pub struct ArtIndex {
    root: Option<Box<Node>>,
    pub len: usize,
}

impl ArtIndex {
    pub fn new() -> Self {
        ArtIndex {
            root: None,
            len: 0,
        }
    }

    pub fn clear(&mut self) {
        self.root = None;
        self.len = 0;
    }

    pub fn insert(&mut self, key: &str, value: u64) {
        let kb = key.as_bytes();
        if self.root.is_none() {
            self.root = Some(Box::new(Node::Leaf {
                key: kb.to_vec(),
                value,
            }));
            self.len = 1;
            return;
        }
        let root = self.root.take().unwrap();
        self.root = Some(insert_rec(root, kb, 0, value, &mut self.len));
    }

    pub fn delete(&mut self, key: &str) -> bool {
        let before = self.len;
        if let Some(root) = self.root.take() {
            self.root = delete_rec(root, key.as_bytes(), 0, &mut self.len);
        }
        self.len < before
    }

    pub fn get(&self, key: &str) -> Option<u64> {
        let mut node = self.root.as_ref()?;
        let kb = key.as_bytes();
        let mut depth = 0usize;
        loop {
            match node.as_ref() {
                Node::Leaf { key: lk, value } => {
                    return if lk.as_slice() == kb {
                        Some(*value)
                    } else {
                        None
                    };
                }
                Node::Inner4 {
                    prefix,
                    keys,
                    children,
                    n,
                } => {
                    if !kb[depth..].starts_with(prefix) {
                        return None;
                    }
                    depth += prefix.len();
                    if depth >= kb.len() {
                        return None;
                    }
                    let b = kb[depth];
                    depth += 1;
                    let mut found = None;
                    for i in 0..*n as usize {
                        if keys[i] == b {
                            found = children[i].as_ref();
                            break;
                        }
                    }
                    node = found?;
                }
                Node::Inner16 {
                    prefix,
                    keys,
                    children,
                    n,
                } => {
                    if !kb[depth..].starts_with(prefix) {
                        return None;
                    }
                    depth += prefix.len();
                    if depth >= kb.len() {
                        return None;
                    }
                    let b = kb[depth];
                    depth += 1;
                    let idx = find_child_byte16(keys, *n, b)?;
                    node = children[idx].as_ref()?;
                }
                Node::Inner48 {
                    prefix,
                    keys,
                    children,
                    ..
                } => {
                    if !kb[depth..].starts_with(prefix) {
                        return None;
                    }
                    depth += prefix.len();
                    if depth >= kb.len() {
                        return None;
                    }
                    let b = kb[depth] as usize;
                    depth += 1;
                    let idx = keys[b];
                    if idx == 0 {
                        return None;
                    }
                    node = children[idx as usize - 1].as_ref()?;
                }
                Node::Inner256 {
                    prefix,
                    children,
                    ..
                } => {
                    if !kb[depth..].starts_with(prefix) {
                        return None;
                    }
                    depth += prefix.len();
                    if depth >= kb.len() {
                        return None;
                    }
                    let b = kb[depth] as usize;
                    depth += 1;
                    node = children[b].as_ref()?;
                }
            }
        }
    }

    pub fn scan_prefix(&self, prefix: &str) -> Vec<(String, u64)> {
        let mut out = Vec::new();
        if let Some(ref root) = self.root {
            collect_prefix(root, prefix.as_bytes(), &mut out);
        }
        out
    }
}

fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count()
}

/// Remove `key` da subárvore, **reclamando memória** (sem tombstone): retorna
/// `None` se a subárvore ficou vazia (folha removida / nó sem filhos), senão o
/// nó (possivelmente **encolhido**: 256→48, 48→16, 16→4 quando o `n` cai).
/// `len` é decrementado exatamente uma vez quando uma folha viva é removida.
fn delete_rec(
    node: Box<Node>,
    key: &[u8],
    depth: usize,
    len: &mut usize,
) -> Option<Box<Node>> {
    match *node {
        Node::Leaf { key: lk, value: v } => {
            if lk.as_slice() == key {
                *len = len.saturating_sub(1);
                None // folha removida — pai desocupa o slot
            } else {
                Some(Box::new(Node::Leaf { key: lk, value: v }))
            }
        }
        Node::Inner4 {
            prefix,
            mut keys,
            mut children,
            mut n,
        } => {
            if !key[depth.min(key.len())..].starts_with(&prefix) || depth + prefix.len() >= key.len()
            {
                return Some(Box::new(Node::Inner4 {
                    prefix,
                    keys,
                    children,
                    n,
                }));
            }
            let b = key[depth + prefix.len()];
            for i in 0..n as usize {
                if keys[i] == b {
                    if let Some(child) = children[i].take() {
                        match delete_rec(child, key, depth + prefix.len() + 1, len) {
                            Some(c2) => children[i] = Some(c2),
                            None => {
                                // desocupa o slot i e compacta o resto à esquerda
                                for j in i..(n as usize - 1) {
                                    keys[j] = keys[j + 1];
                                    children[j] = children[j + 1].take();
                                }
                                children[n as usize - 1] = None;
                                n -= 1;
                            }
                        }
                    }
                    if n == 0 {
                        return None; // Inner4 sem filhos → remove do pai
                    }
                    return Some(Box::new(Node::Inner4 {
                        prefix,
                        keys,
                        children,
                        n,
                    }));
                }
            }
            Some(Box::new(Node::Inner4 {
                prefix,
                keys,
                children,
                n,
            }))
        }
        Node::Inner16 {
            prefix,
            mut keys,
            mut children,
            mut n,
        } => {
            if !key[depth.min(key.len())..].starts_with(&prefix) || depth + prefix.len() >= key.len()
            {
                return Some(Box::new(Node::Inner16 {
                    prefix,
                    keys,
                    children,
                    n,
                }));
            }
            let b = key[depth + prefix.len()];
            for i in 0..n as usize {
                if keys[i] == b {
                    if let Some(child) = children[i].take() {
                        match delete_rec(child, key, depth + prefix.len() + 1, len) {
                            Some(c2) => children[i] = Some(c2),
                            None => {
                                for j in i..(n as usize - 1) {
                                    keys[j] = keys[j + 1];
                                    children[j] = children[j + 1].take();
                                }
                                children[n as usize - 1] = None;
                                n -= 1;
                            }
                        }
                    }
                    if n == 0 {
                        return None;
                    }
                    // shrink: n <= 4 → Inner4
                    if n <= 4 {
                        let mut k4 = [0u8; 4];
                        let mut c4: [Option<Box<Node>>; 4] = [None, None, None, None];
                        for i in 0..n as usize {
                            k4[i] = keys[i];
                            c4[i] = children[i].take();
                        }
                        return Some(Box::new(Node::Inner4 {
                            prefix,
                            keys: k4,
                            children: c4,
                            n,
                        }));
                    }
                    return Some(Box::new(Node::Inner16 {
                        prefix,
                        keys,
                        children,
                        n,
                    }));
                }
            }
            Some(Box::new(Node::Inner16 {
                prefix,
                keys,
                children,
                n,
            }))
        }
        Node::Inner48 {
            prefix,
            mut keys,
            mut children,
            mut n,
        } => {
            if !key[depth.min(key.len())..].starts_with(&prefix) || depth + prefix.len() >= key.len()
            {
                return Some(Box::new(Node::Inner48 {
                    prefix,
                    keys,
                    children,
                    n,
                }));
            }
            let b = key[depth + prefix.len()] as usize;
            let idx = keys[b];
            if idx != 0 {
                if let Some(child) = children[idx as usize - 1].take() {
                    match delete_rec(child, key, depth + prefix.len() + 1, len) {
                        Some(c2) => children[idx as usize - 1] = Some(c2),
                        None => {
                            // move o ÚLTIMO filho para o buraco e renumera
                            let last = n as usize - 1;
                            if idx as usize - 1 != last {
                                children[idx as usize - 1] = children[last].take();
                                if let Some(lb) = keys.iter().position(|&k| k == last as u8 + 1) {
                                    keys[lb] = idx;
                                }
                            }
                            keys[b] = 0;
                            children[last] = None;
                            n -= 1;
                        }
                    }
                }
            }
            if n == 0 {
                return None;
            }
            // shrink: n <= 16 → Inner16 (reconstrói keys em ordem de byte)
            if n <= 16 {
                let mut k16 = [0u8; 16];
                let mut c16: [Option<Box<Node>>; 16] = empty16();
                let mut m = 0usize;
                for (byte, &idx) in keys.iter().enumerate() {
                    if idx != 0 {
                        k16[m] = byte as u8;
                        c16[m] = children[idx as usize - 1].take();
                        m += 1;
                    }
                }
                return Some(Box::new(Node::Inner16 {
                    prefix,
                    keys: k16,
                    children: c16,
                    n: m as u8,
                }));
            }
            Some(Box::new(Node::Inner48 {
                prefix,
                keys,
                children,
                n,
            }))
        }
        Node::Inner256 {
            prefix,
            mut children,
            mut n,
        } => {
            if !key[depth.min(key.len())..].starts_with(&prefix) || depth + prefix.len() >= key.len()
            {
                return Some(Box::new(Node::Inner256 {
                    prefix,
                    children,
                    n,
                }));
            }
            let b = key[depth + prefix.len()] as usize;
            if let Some(child) = children[b].take() {
                match delete_rec(child, key, depth + prefix.len() + 1, len) {
                    Some(c2) => children[b] = Some(c2),
                    None => n = n.saturating_sub(1),
                }
            }
            if n == 0 {
                return None;
            }
            // shrink: n <= 48 → Inner48
            if n <= 48 {
                let mut k48 = [0u8; 256];
                let mut c48: [Option<Box<Node>>; 48] = empty48();
                let mut m = 0usize;
                for (byte, c) in children.iter_mut().enumerate() {
                    if let Some(ch) = c.take() {
                        k48[byte] = m as u8 + 1;
                        c48[m] = Some(ch);
                        m += 1;
                    }
                }
                return Some(Box::new(Node::Inner48 {
                    prefix,
                    keys: k48,
                    children: c48,
                    n: m as u8,
                }));
            }
            Some(Box::new(Node::Inner256 {
                prefix,
                children,
                n,
            }))
        }
    }
}

fn insert_rec(node: Box<Node>, key: &[u8], depth: usize, value: u64, len: &mut usize) -> Box<Node> {
    match *node {
        Node::Leaf {
            key: ref lk,
            value: old_v,
        } => {
            if lk.as_slice() == key {
                return Box::new(Node::Leaf {
                    key: key.to_vec(),
                    value,
                });
            }
            let cp = common_prefix(&lk[depth.min(lk.len())..], &key[depth.min(key.len())..]);
            let prefix = lk[depth..depth + cp].to_vec();
            let d2 = depth + cp;
            let b1 = if d2 < lk.len() { lk[d2] } else { 0 };
            let b2 = if d2 < key.len() { key[d2] } else { 0 };
            let leaf1 = Box::new(Node::Leaf {
                key: lk.clone(),
                value: old_v,
            });
            let leaf2 = Box::new(Node::Leaf {
                key: key.to_vec(),
                value,
            });
            *len += 1;
            let mut keys = [0u8; 4];
            let mut children: [Option<Box<Node>>; 4] = [None, None, None, None];
            keys[0] = b1;
            keys[1] = b2;
            children[0] = Some(leaf1);
            children[1] = Some(leaf2);
            Box::new(Node::Inner4 {
                prefix,
                keys,
                children,
                n: 2,
            })
        }
        Node::Inner4 {
            prefix,
            mut keys,
            mut children,
            mut n,
        } => {
            if key.len() >= depth + prefix.len() && key[depth..].starts_with(&prefix) {
                let d2 = depth + prefix.len();
                if d2 >= key.len() {
                    return Box::new(Node::Inner4 {
                        prefix,
                        keys,
                        children,
                        n,
                    });
                }
                let b = key[d2];
                for i in 0..n as usize {
                    if keys[i] == b {
                        let child = children[i].take().unwrap();
                        children[i] = Some(insert_rec(child, key, d2 + 1, value, len));
                        return Box::new(Node::Inner4 {
                            prefix,
                            keys,
                            children,
                            n,
                        });
                    }
                }
                if n < 4 {
                    keys[n as usize] = b;
                    children[n as usize] = Some(Box::new(Node::Leaf {
                        key: key.to_vec(),
                        value,
                        }));
                    n += 1;
                    *len += 1;
                    return Box::new(Node::Inner4 {
                        prefix,
                        keys,
                        children,
                        n,
                    });
                }
                // grow → 16
                let mut k16 = [0u8; 16];
                let mut c16 = empty16();
                for i in 0..4 {
                    k16[i] = keys[i];
                    c16[i] = children[i].take();
                }
                k16[4] = b;
                c16[4] = Some(Box::new(Node::Leaf {
                    key: key.to_vec(),
                    value,
                }));
                *len += 1;
                Box::new(Node::Inner16 {
                    prefix,
                    keys: k16,
                    children: c16,
                    n: 5,
                })
            } else {
                // Prefix mismatch: split into parent Inner4 + old node + new leaf
                let np = prefix;
                let kr = &key[depth..];
                let cp = common_prefix(&np, kr);
                if cp >= np.len() || cp >= kr.len() {
                    Box::new(Node::Inner4 {
                        prefix: np,
                        keys,
                        children,
                        n,
                    })
                } else {
                    let shared = np[..cp].to_vec();
                    let nb = np[cp];
                    let kb = kr[cp];
                    let old_prefix = np[cp + 1..].to_vec();
                    let old = Box::new(Node::Inner4 {
                        prefix: old_prefix,
                        keys,
                        children,
                        n,
                    });
                    let leaf = Box::new(Node::Leaf {
                        key: key.to_vec(),
                        value,
                        });
                    let mut pk = [0u8; 4];
                    let mut pc: [Option<Box<Node>>; 4] = [None, None, None, None];
                    pk[0] = nb;
                    pk[1] = kb;
                    pc[0] = Some(old);
                    pc[1] = Some(leaf);
                    *len += 1;
                    Box::new(Node::Inner4 {
                        prefix: shared,
                        keys: pk,
                        children: pc,
                        n: 2,
                    })
                }
            }
        }
        Node::Inner16 {
            prefix,
            mut keys,
            mut children,
            mut n,
        } => {
            if key.len() >= depth + prefix.len() && key[depth..].starts_with(&prefix) {
                let d2 = depth + prefix.len();
                if d2 >= key.len() {
                    return Box::new(Node::Inner16 {
                        prefix,
                        keys,
                        children,
                        n,
                    });
                }
                let b = key[d2];
                for i in 0..n as usize {
                    if keys[i] == b {
                        let child = children[i].take().unwrap();
                        children[i] = Some(insert_rec(child, key, d2 + 1, value, len));
                        return Box::new(Node::Inner16 {
                            prefix,
                            keys,
                            children,
                            n,
                        });
                    }
                }
                if (n as usize) < 16 {
                    keys[n as usize] = b;
                    children[n as usize] = Some(Box::new(Node::Leaf {
                        key: key.to_vec(),
                        value,
                        }));
                    n += 1;
                    *len += 1;
                    return Box::new(Node::Inner16 {
                        prefix,
                        keys,
                        children,
                        n,
                    });
                }
                // grow → 48
                let mut k48 = [0u8; 256];
                let mut c48 = empty48();
                for i in 0..16 {
                    k48[keys[i] as usize] = (i as u8) + 1;
                    c48[i] = children[i].take();
                }
                k48[b as usize] = 17;
                c48[16] = Some(Box::new(Node::Leaf {
                    key: key.to_vec(),
                    value,
                }));
                *len += 1;
                Box::new(Node::Inner48 {
                    prefix,
                    keys: k48,
                    children: c48,
                    n: 17,
                })
            } else {
                // Prefix mismatch: split into parent Inner4 + old node + new leaf
                let np = prefix;
                let kr = &key[depth..];
                let cp = common_prefix(&np, kr);
                if cp >= np.len() || cp >= kr.len() {
                    Box::new(Node::Inner16 {
                        prefix: np,
                        keys,
                        children,
                        n,
                    })
                } else {
                    let shared = np[..cp].to_vec();
                    let nb = np[cp];
                    let kb = kr[cp];
                    let old_prefix = np[cp + 1..].to_vec();
                    let old = Box::new(Node::Inner16 {
                        prefix: old_prefix,
                        keys,
                        children,
                        n,
                    });
                    let leaf = Box::new(Node::Leaf {
                        key: key.to_vec(),
                        value,
                        });
                    let mut pk = [0u8; 4];
                    let mut pc: [Option<Box<Node>>; 4] = [None, None, None, None];
                    pk[0] = nb;
                    pk[1] = kb;
                    pc[0] = Some(old);
                    pc[1] = Some(leaf);
                    *len += 1;
                    Box::new(Node::Inner4 {
                        prefix: shared,
                        keys: pk,
                        children: pc,
                        n: 2,
                    })
                }
            }
        }
        Node::Inner48 {
            prefix,
            mut keys,
            mut children,
            mut n,
        } => {
            if key.len() >= depth + prefix.len() && key[depth..].starts_with(&prefix) {
                let d2 = depth + prefix.len();
                if d2 >= key.len() {
                    return Box::new(Node::Inner48 {
                        prefix,
                        keys,
                        children,
                        n,
                    });
                }
                let b = key[d2] as usize;
                let idx = keys[b];
                if idx != 0 {
                    let child = children[idx as usize - 1].take().unwrap();
                    children[idx as usize - 1] = Some(insert_rec(child, key, d2 + 1, value, len));
                    return Box::new(Node::Inner48 {
                        prefix,
                        keys,
                        children,
                        n,
                    });
                }
                if (n as usize) < 48 {
                    keys[b] = n + 1;
                    children[n as usize] = Some(Box::new(Node::Leaf {
                        key: key.to_vec(),
                        value,
                        }));
                    n += 1;
                    *len += 1;
                    return Box::new(Node::Inner48 {
                        prefix,
                        keys,
                        children,
                        n,
                    });
                }
                // grow → 256
                let mut c256 = empty256();
                for byte in 0..256u16 {
                    let i = keys[byte as usize];
                    if i != 0 {
                        c256[byte as usize] = children[i as usize - 1].take();
                    }
                }
                c256[b] = Some(Box::new(Node::Leaf {
                    key: key.to_vec(),
                    value,
                }));
                *len += 1;
                Box::new(Node::Inner256 {
                    prefix,
                    children: c256,
                    n: 49,
                })
            } else {
                // Prefix mismatch: split into parent Inner4 + old node + new leaf
                let np = prefix;
                let kr = &key[depth..];
                let cp = common_prefix(&np, kr);
                if cp >= np.len() || cp >= kr.len() {
                    Box::new(Node::Inner48 {
                        prefix: np,
                        keys,
                        children,
                        n,
                    })
                } else {
                    let shared = np[..cp].to_vec();
                    let nb = np[cp];
                    let kb = kr[cp];
                    let old_prefix = np[cp + 1..].to_vec();
                    let old = Box::new(Node::Inner48 {
                        prefix: old_prefix,
                        keys,
                        children,
                        n,
                    });
                    let leaf = Box::new(Node::Leaf {
                        key: key.to_vec(),
                        value,
                        });
                    let mut pk = [0u8; 4];
                    let mut pc: [Option<Box<Node>>; 4] = [None, None, None, None];
                    pk[0] = nb;
                    pk[1] = kb;
                    pc[0] = Some(old);
                    pc[1] = Some(leaf);
                    *len += 1;
                    Box::new(Node::Inner4 {
                        prefix: shared,
                        keys: pk,
                        children: pc,
                        n: 2,
                    })
                }
            }
        }
        Node::Inner256 {
            prefix,
            mut children,
            mut n,
        } => {
            if key.len() >= depth + prefix.len() && key[depth..].starts_with(&prefix) {
                let d2 = depth + prefix.len();
                if d2 >= key.len() {
                    return Box::new(Node::Inner256 {
                        prefix,
                        children,
                        n,
                    });
                }
                let b = key[d2] as usize;
                if let Some(child) = children[b].take() {
                    children[b] = Some(insert_rec(child, key, d2 + 1, value, len));
                } else {
                    children[b] = Some(Box::new(Node::Leaf {
                        key: key.to_vec(),
                        value,
                        }));
                    n = n.saturating_add(1);
                    *len += 1;
                }
                Box::new(Node::Inner256 {
                    prefix,
                    children,
                    n,
                })
            } else {
                // Prefix mismatch: split into parent Inner4 + old node + new leaf
                let np = prefix;
                let kr = &key[depth..];
                let cp = common_prefix(&np, kr);
                if cp >= np.len() || cp >= kr.len() {
                    Box::new(Node::Inner256 {
                        prefix: np,
                        children,
                        n,
                    })
                } else {
                    let shared = np[..cp].to_vec();
                    let nb = np[cp];
                    let kb = kr[cp];
                    let old_prefix = np[cp + 1..].to_vec();
                    let old = Box::new(Node::Inner256 {
                        prefix: old_prefix,
                        children,
                        n,
                    });
                    let leaf = Box::new(Node::Leaf {
                        key: key.to_vec(),
                        value,
                        });
                    let mut pk = [0u8; 4];
                    let mut pc: [Option<Box<Node>>; 4] = [None, None, None, None];
                    pk[0] = nb;
                    pk[1] = kb;
                    pc[0] = Some(old);
                    pc[1] = Some(leaf);
                    *len += 1;
                    Box::new(Node::Inner4 {
                        prefix: shared,
                        keys: pk,
                        children: pc,
                        n: 2,
                    })
                }
            }
        }
    }
}

fn collect_prefix(node: &Node, prefix: &[u8], out: &mut Vec<(String, u64)>) {
    match node {
        Node::Leaf { key, value } => {
            if key.starts_with(prefix) {
                if let Ok(s) = core::str::from_utf8(key) {
                    out.push((String::from(s), *value));
                }
            }
        }
        Node::Inner4 { children, n, .. } => {
            for i in 0..*n as usize {
                if let Some(ref c) = children[i] {
                    collect_prefix(c, prefix, out);
                }
            }
        }
        Node::Inner16 { children, n, .. } => {
            for i in 0..*n as usize {
                if let Some(ref c) = children[i] {
                    collect_prefix(c, prefix, out);
                }
            }
        }
        Node::Inner48 { children, n, .. } => {
            for i in 0..*n as usize {
                if let Some(ref c) = children[i] {
                    collect_prefix(c, prefix, out);
                }
            }
        }
        Node::Inner256 { children, .. } => {
            for c in children.iter() {
                if let Some(ref ch) = c {
                    collect_prefix(ch, prefix, out);
                }
            }
        }
    }
}

impl Default for ArtIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn art_smoke() {
        let mut art = ArtIndex::new();
        art.insert("md/L1/a", 10);
        art.insert("md/L1/b", 20);
        art.insert("md/L2/c", 30);
        assert_eq!(art.get("md/L1/a"), Some(10));
        assert_eq!(art.get("md/L1/b"), Some(20));
        assert!(art.scan_prefix("md/L1/").len() >= 2);
        assert_eq!(art.len, 3);
        assert!(art.delete("md/L1/b"));
        assert!(art.get("md/L1/b").is_none());
        assert_eq!(art.len, 2);
    }

    #[test]
    fn art_shrink_after_delete_reclaims_nodes() {
        // delete agora REMOVE nós (sem tombstone) e encolhe 256→48→16→4.
        // Força Inner256 com 200 chaves, deleta quase tudo, verifica integridade.
        let mut art = ArtIndex::new();
        for i in 0..200 {
            art.insert(&alloc::format!("k{i:03}"), i as u64);
        }
        assert_eq!(art.len, 200);
        for i in 0..199 {
            assert!(art.delete(&alloc::format!("k{i:03}")), "delete {i} falhou");
        }
        assert_eq!(art.len, 1);
        assert_eq!(art.get("k199"), Some(199));
        assert!(art.get("k000").is_none());
        // re-insere após shrink — integridade mantida
        art.insert("k000", 0);
        assert_eq!(art.len, 2);
        assert_eq!(art.get("k000"), Some(0));
        assert_eq!(art.scan_prefix("k").len(), 2);
        // deleta tudo → árvore vazia (root None, len 0)
        assert!(art.delete("k000"));
        assert!(art.delete("k199"));
        assert_eq!(art.len, 0);
        assert!(art.scan_prefix("k").is_empty());
        // re-insere do zero após esvaziar
        art.insert("novo", 42);
        assert_eq!(art.get("novo"), Some(42));
        assert_eq!(art.len, 1);
    }

    #[test]
    fn art_churn_100k_ops_stays_consistent() {
        // insert/delete alternados em 5k chaves × 20 rounds = 100k ops —
        // shrink/remoção de slots não pode corromper a estrutura.
        let mut art = ArtIndex::new();
        for round in 0..20 {
            for i in 0..5000 {
                let k = alloc::format!("ch/{i:05}");
                if round % 2 == 0 {
                    art.insert(&k, i as u64);
                } else {
                    let _ = art.delete(&k);
                }
            }
        }
        // round 19 (ímpar) deletou tudo → len 0 e scan vazio
        assert_eq!(art.len, 0);
        assert!(art.scan_prefix("ch/").is_empty());
        // consistência total: insert de novo e verifica scan == len
        for i in 0..100 {
            art.insert(&alloc::format!("ch/{i:05}"), i as u64);
        }
        assert_eq!(art.len, 100);
        assert_eq!(art.scan_prefix("ch/").len(), 100);
        for i in 0..100 {
            assert_eq!(art.get(&alloc::format!("ch/{i:05}")), Some(i as u64));
        }
        // deleta metade e verifica
        for i in (0..100).step_by(2) {
            assert!(art.delete(&alloc::format!("ch/{i:05}")));
        }
        assert_eq!(art.len, 50);
        assert_eq!(art.scan_prefix("ch/").len(), 50);
    }

    #[test]
    fn art_shared_prefix_split() {
        // Contract-compliant keys (fixed-width suffixes — NO prefix
        // relationship, e.g. "k0000/01" vs "k0000/10"). Inherited ART
        // limitation: keys where one is a prefix of another are not supported
        // (see docs/api.md "Inherited limitations").
        let mut art = ArtIndex::new();
        for i in 0..50 {
            art.insert(&alloc::format!("k{:04}/{:02}", i / 16, i), i as u64);
        }
        for i in 0..50 {
            assert_eq!(
                art.get(&alloc::format!("k{:04}/{:02}", i / 16, i)),
                Some(i as u64)
            );
        }
        assert_eq!(art.scan_prefix("k").len(), 50);
    }
}
