// SPDX-License-Identifier: GPL-3.0-or-later
// Port of WSJT-X fano.c: Copyright 1994 Phil Karn, KA9Q;
// modifications by Joe Taylor, K1JT.
use super::tables::{METRIC, SYNC};
const POLY1: u32 = 0xf2d05351;
const POLY2: u32 = 0xe4613c47;
fn symbol(state: u32) -> usize {
    (((state & POLY1).count_ones() & 1) * 2 + ((state & POLY2).count_ones() & 1)) as usize
}
pub fn encode(payload: &[u8; 50]) -> [u8; 162] {
    let mut encoded = [0; 162];
    let mut state = 0u32;
    for i in 0..81 {
        state = (state << 1) | if i < 50 { payload[i] as u32 } else { 0 };
        let s = symbol(state);
        encoded[2 * i] = (s >> 1) as u8;
        encoded[2 * i + 1] = (s & 1) as u8;
    }
    encoded
}
pub fn tones(payload: &[u8; 50]) -> [u8; 162] {
    let encoded = encode(payload);
    let mut out = SYNC;
    let mut p = 0;
    for i in 0u16..256 {
        let j = (i as u8).reverse_bits() as usize;
        if j < 162 {
            out[j] += 2 * encoded[p];
            p += 1;
        }
    }
    out
}
pub fn deinterleave(input: &[u8; 162]) -> [u8; 162] {
    let mut out = [128; 162];
    let mut p = 0;
    for i in 0u16..256 {
        let j = (i as u8).reverse_bits() as usize;
        if j < 162 {
            out[p] = input[j];
            p += 1;
        }
    }
    out
}
#[derive(Clone, Copy, Default)]
struct Node {
    state: u32,
    gamma: i32,
    metrics: [i32; 4],
    sorted: [i32; 2],
    branch: usize,
}
fn prepare(node: &mut Node, tail: bool) {
    let s = symbol(node.state);
    let m0 = node.metrics[s];
    let m1 = node.metrics[s ^ 3];
    if !tail && m1 >= m0 {
        node.sorted = [m1, m0];
        node.state |= 1;
    } else {
        node.sorted = [m0, m1];
    }
    node.branch = 0;
}
/// Same bounded sequential tree search and metric table as wsprd's Fano path.
pub fn decode(symbols: &[u8; 162], cycles_per_bit: usize) -> Option<[u8; 50]> {
    let mut nodes = [Node::default(); 82];
    let metric = |sent: usize, received: u8| -> i32 {
        let index = if sent == 0 {
            received as usize
        } else {
            255 - received as usize
        };
        (10.0 * (METRIC[index] - 0.45)).round() as i32
    };
    for i in 0..81 {
        let a = symbols[2 * i];
        let b = symbols[2 * i + 1];
        nodes[i].metrics = std::array::from_fn(|s| metric(s >> 1, a) + metric(s & 1, b));
    }
    prepare(&mut nodes[0], false);
    let mut position = 0usize;
    let mut threshold = 0;
    for _ in 0..cycles_per_bit * 81 {
        let gamma = nodes[position].gamma + nodes[position].sorted[nodes[position].branch];
        if gamma >= threshold {
            if nodes[position].gamma < threshold + 60 {
                while gamma >= threshold + 60 {
                    threshold += 60;
                }
            }
            let state = nodes[position].state << 1;
            position += 1;
            nodes[position].gamma = gamma;
            nodes[position].state = state;
            if position == 81 {
                let result = std::array::from_fn(|i| (nodes[i].state & 1) as u8);
                return Some(result);
            }
            prepare(&mut nodes[position], position >= 50);
            continue;
        }
        loop {
            if position == 0 || nodes[position - 1].gamma < threshold {
                threshold -= 60;
                if nodes[position].branch != 0 {
                    nodes[position].branch = 0;
                    nodes[position].state ^= 1;
                }
                break;
            }
            position -= 1;
            if position < 50 && nodes[position].branch != 1 {
                nodes[position].branch = 1;
                nodes[position].state ^= 1;
                break;
            }
        }
    }
    None
}
