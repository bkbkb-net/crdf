//! Moving between the two ways to hold the same 64 lanes.
//!
//! The evaluators hold a circuit's state **bit-plane major**: one `u64`
//! per wire, whose bit *l* is that wire's value in lane *l*. A NAND is
//! then one `and` and one `not` for all 64 lanes at once, which is the
//! whole reason the representation was chosen.
//!
//! A native machine instruction wants the opposite layout. `mul`,
//! `fmul`, `add` — anything that treats a group of wires as one number —
//! needs the number's bits gathered into a single register, **lane
//! major**: one word per lane, holding all of that lane's bits.
//!
//! Neither layout is right in general, so a compiler that wants to
//! replace a sub-circuit with a native instruction has to convert at the
//! boundary and convert back. That conversion is a bit-matrix
//! transpose, and its cost decides whether the substitution is worth
//! doing at all. This module exists so the cost is a measured number
//! rather than an estimate.
//!
//! Nothing here knows what the bits mean.

/// Transpose a 32x32 bit matrix in place.
///
/// `a[r]` bit `c` becomes `a[c]` bit `r`. The recursive block exchange
/// from Hacker's Delight, mirrored: the published form indexes columns
/// from the most significant bit, so taken literally it transposes the
/// matrix *and* reverses both axes. Shifting the other way and starting
/// the mask from the high half gives the plain transpose, which is the
/// one that matches how lanes are numbered here.
///
/// Five passes over a halving block size, so about 160 operations
/// rather than the 1,024 a bit-at-a-time loop would take.
fn transpose32(a: &mut [u32; 32]) {
    let mut j = 16;
    let mut m = 0xffff_0000_u32;
    while j != 0 {
        let mut k = 0;
        while k < 32 {
            let t = (a[k] ^ (a[k | j] << j)) & m;
            a[k] ^= t;
            a[k | j] ^= t >> j;
            k = (k + j + 1) & !j;
        }
        j >>= 1;
        m ^= m >> j;
    }
}

/// Bit planes to lane-major words.
///
/// `planes[b]` bit `l` is bit `b` of lane `l`; the result's element `l`
/// is lane `l`'s whole 32-bit value. Done as two independent 32x32
/// transposes, one per half of the 64 lanes.
pub fn planes_to_lanes(planes: &[u64; 32]) -> [u32; 64] {
    let mut lo = [0_u32; 32];
    let mut hi = [0_u32; 32];
    for b in 0..32 {
        lo[b] = planes[b] as u32;
        hi[b] = (planes[b] >> 32) as u32;
    }
    transpose32(&mut lo);
    transpose32(&mut hi);

    let mut out = [0_u32; 64];
    out[..32].copy_from_slice(&lo);
    out[32..].copy_from_slice(&hi);
    out
}

/// Lane-major words back to bit planes. The exact inverse of
/// [`planes_to_lanes`], and the same cost: a transpose is its own
/// undoing.
pub fn lanes_to_planes(lanes: &[u32; 64]) -> [u64; 32] {
    let mut lo = [0_u32; 32];
    let mut hi = [0_u32; 32];
    lo.copy_from_slice(&lanes[..32]);
    hi.copy_from_slice(&lanes[32..]);
    transpose32(&mut lo);
    transpose32(&mut hi);

    let mut planes = [0_u64; 32];
    for b in 0..32 {
        planes[b] = u64::from(lo[b]) | (u64::from(hi[b]) << 32);
    }
    planes
}
