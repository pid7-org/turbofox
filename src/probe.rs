use crate::common::{BUCKET_ENTRIES, HASH_EMPTY, HASH_TOMBSTONE};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SimdProbeMask {
    pub(crate) match_mask: u32,
    pub(crate) empty_mask: u32,
    pub(crate) tomb_mask: u32,
}

#[allow(dead_code)]
pub(crate) type ProbeMatchFn = unsafe fn(&[u32; BUCKET_ENTRIES], u32) -> u32;
pub(crate) type ProbeFullFn = unsafe fn(&[u32; BUCKET_ENTRIES], u32) -> SimdProbeMask;
pub(crate) type ProbeKernelFn = ProbeFullFn;

#[allow(dead_code)]
#[inline(always)]
pub(crate) unsafe fn probe_match_scalar(hashes: &[u32; BUCKET_ENTRIES], target: u32) -> u32 {
    let mut match_mask = 0u32;
    let mut i = 0x00;

    while i < BUCKET_ENTRIES {
        if hashes[i] == target {
            match_mask |= 1 << i;
        }

        i += 0x01;
    }

    match_mask
}

#[inline(always)]
pub(crate) unsafe fn probe_full_scalar(
    hashes: &[u32; BUCKET_ENTRIES],
    target: u32,
) -> SimdProbeMask {
    let mut match_mask = 0u32;
    let mut empty_mask = 0u32;
    let mut tomb_mask = 0u32;

    let mut i = 0x00;
    while i < BUCKET_ENTRIES {
        let h = hashes[i];

        if h == target {
            match_mask |= 1 << i;
        }

        if h == HASH_EMPTY {
            empty_mask |= 1 << i;
        } else if h == HASH_TOMBSTONE {
            tomb_mask |= 1 << i;
        }

        i += 0x01;
    }

    SimdProbeMask { match_mask, empty_mask, tomb_mask }
}

#[inline(always)]
#[allow(dead_code)]
pub(crate) unsafe fn open_address_scalar(
    hashes: &[u32; BUCKET_ENTRIES],
    target: u32,
) -> SimdProbeMask {
    probe_full_scalar(hashes, target)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
pub(crate) unsafe fn probe_match_sse2(hashes: &[u32; BUCKET_ENTRIES], target: u32) -> u32 {
    use core::arch::x86_64::*;

    let ptr = hashes.as_ptr() as *const __m128i;
    let v_target = _mm_set1_epi32(target as i32);

    let h0 = _mm_loadu_si128(ptr);
    let h1 = _mm_loadu_si128(ptr.add(0x01));
    let h2 = _mm_loadu_si128(ptr.add(0x02));
    let h3 = _mm_loadu_si128(ptr.add(0x03));
    let h4 = _mm_loadu_si128(ptr.add(0x04));

    let m0 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h0, v_target)));
    let m1 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h1, v_target)));
    let m2 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h2, v_target)));
    let m3 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h3, v_target)));
    let m4 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h4, v_target)));
    let mut match_mask = (m0 | (m1 << 4) | (m2 << 8) | (m3 << 12) | (m4 << 16)) as u32;

    let m20 = ((hashes[20] == target) as u32) << 20;
    match_mask |= m20;

    match_mask
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
pub(crate) unsafe fn probe_full_sse2(hashes: &[u32; BUCKET_ENTRIES], target: u32) -> SimdProbeMask {
    use core::arch::x86_64::*;

    let ptr = hashes.as_ptr() as *const __m128i;

    let v_empty = _mm_setzero_si128();
    let v_target = _mm_set1_epi32(target as i32);
    let v_tomb = _mm_set1_epi32(HASH_TOMBSTONE as i32);

    let h0 = _mm_loadu_si128(ptr);
    let h1 = _mm_loadu_si128(ptr.add(0x01));
    let h2 = _mm_loadu_si128(ptr.add(0x02));
    let h3 = _mm_loadu_si128(ptr.add(0x03));
    let h4 = _mm_loadu_si128(ptr.add(0x04));

    let m0 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h0, v_target)));
    let m1 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h1, v_target)));
    let m2 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h2, v_target)));
    let m3 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h3, v_target)));
    let m4 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h4, v_target)));
    let mut match_mask = (m0 | (m1 << 4) | (m2 << 8) | (m3 << 12) | (m4 << 16)) as u32;

    let e0 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h0, v_empty)));
    let e1 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h1, v_empty)));
    let e2 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h2, v_empty)));
    let e3 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h3, v_empty)));
    let e4 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h4, v_empty)));
    let mut empty_mask = (e0 | (e1 << 4) | (e2 << 8) | (e3 << 12) | (e4 << 16)) as u32;

    let t0 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h0, v_tomb)));
    let t1 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h1, v_tomb)));
    let t2 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h2, v_tomb)));
    let t3 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h3, v_tomb)));
    let t4 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h4, v_tomb)));
    let mut tomb_mask = (t0 | (t1 << 4) | (t2 << 8) | (t3 << 12) | (t4 << 16)) as u32;

    let m20 = ((hashes[20] == target) as u32) << 20;
    match_mask |= m20;

    let e20 = ((hashes[20] == HASH_EMPTY) as u32) << 20;
    empty_mask |= e20;

    let t20 = ((hashes[20] == HASH_TOMBSTONE) as u32) << 20;
    tomb_mask |= t20;

    SimdProbeMask { match_mask, empty_mask, tomb_mask }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
#[allow(dead_code)]
pub(crate) unsafe fn open_address_sse2(
    hashes: &[u32; BUCKET_ENTRIES],
    target: u32,
) -> SimdProbeMask {
    probe_full_sse2(hashes, target)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn probe_match_avx2(hashes: &[u32; BUCKET_ENTRIES], target: u32) -> u32 {
    use core::arch::x86_64::*;

    let ptr = hashes.as_ptr() as *const __m256i;
    let v_target = _mm256_set1_epi32(target as i32);

    let h0 = _mm256_loadu_si256(ptr);
    let h1 = _mm256_loadu_si256(ptr.add(0x01));

    let m0 = _mm256_movemask_ps(_mm256_castsi256_ps(_mm256_cmpeq_epi32(h0, v_target)));
    let m1 = _mm256_movemask_ps(_mm256_castsi256_ps(_mm256_cmpeq_epi32(h1, v_target)));
    let mut match_mask = ((m0 & 0xFF) | ((m1 & 0xFF) << 8)) as u32;

    let sse_ptr = hashes.as_ptr().add(16) as *const __m128i;
    let v_target_sse = _mm_set1_epi32(target as i32);
    let h2 = _mm_loadu_si128(sse_ptr);
    let m2 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h2, v_target_sse)));
    match_mask |= ((m2 & 0x0F) << 16) as u32;

    let m20 = ((hashes[20] == target) as u32) << 20;
    match_mask |= m20;

    match_mask
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn probe_full_avx2(hashes: &[u32; BUCKET_ENTRIES], target: u32) -> SimdProbeMask {
    use core::arch::x86_64::*;

    let ptr = hashes.as_ptr() as *const __m256i;

    let v_empty = _mm256_setzero_si256();
    let v_target = _mm256_set1_epi32(target as i32);
    let v_tomb = _mm256_set1_epi32(HASH_TOMBSTONE as i32);

    let h0 = _mm256_loadu_si256(ptr);
    let h1 = _mm256_loadu_si256(ptr.add(0x01));

    let m0 = _mm256_movemask_ps(_mm256_castsi256_ps(_mm256_cmpeq_epi32(h0, v_target)));
    let m1 = _mm256_movemask_ps(_mm256_castsi256_ps(_mm256_cmpeq_epi32(h1, v_target)));
    let mut match_mask = ((m0 & 0xFF) | ((m1 & 0xFF) << 8)) as u32;

    let e0 = _mm256_movemask_ps(_mm256_castsi256_ps(_mm256_cmpeq_epi32(h0, v_empty)));
    let e1 = _mm256_movemask_ps(_mm256_castsi256_ps(_mm256_cmpeq_epi32(h1, v_empty)));
    let mut empty_mask = ((e0 & 0xFF) | ((e1 & 0xFF) << 8)) as u32;

    let t0 = _mm256_movemask_ps(_mm256_castsi256_ps(_mm256_cmpeq_epi32(h0, v_tomb)));
    let t1 = _mm256_movemask_ps(_mm256_castsi256_ps(_mm256_cmpeq_epi32(h1, v_tomb)));
    let mut tomb_mask = ((t0 & 0xFF) | ((t1 & 0xFF) << 8)) as u32;

    let sse_ptr = hashes.as_ptr().add(16) as *const __m128i;
    let v_target_sse = _mm_set1_epi32(target as i32);
    let v_empty_sse = _mm_setzero_si128();
    let v_tomb_sse = _mm_set1_epi32(HASH_TOMBSTONE as i32);

    let h2 = _mm_loadu_si128(sse_ptr);
    let m2 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h2, v_target_sse)));
    let e2 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h2, v_empty_sse)));
    let t2 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h2, v_tomb_sse)));

    match_mask |= ((m2 & 0x0F) << 16) as u32;
    empty_mask |= ((e2 & 0x0F) << 16) as u32;
    tomb_mask |= ((t2 & 0x0F) << 16) as u32;

    let m20 = ((hashes[20] == target) as u32) << 20;
    match_mask |= m20;

    let e20 = ((hashes[20] == HASH_EMPTY) as u32) << 20;
    empty_mask |= e20;

    let t20 = ((hashes[20] == HASH_TOMBSTONE) as u32) << 20;
    tomb_mask |= t20;

    SimdProbeMask { match_mask, empty_mask, tomb_mask }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[allow(dead_code)]
pub(crate) unsafe fn open_address_avx2(
    hashes: &[u32; BUCKET_ENTRIES],
    target: u32,
) -> SimdProbeMask {
    probe_full_avx2(hashes, target)
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
pub(crate) unsafe fn probe_match_neon(hashes: &[u32; BUCKET_ENTRIES], target: u32) -> u32 {
    use core::arch::aarch64::*;

    let ptr = hashes.as_ptr();
    let v_target = vdupq_n_u32(target);
    let weights = vld1q_u32([0x01, 0x02, 0x04, 0x08].as_ptr());

    #[inline(always)]
    unsafe fn mask4_fast(v: uint32x4_t, w: uint32x4_t) -> u32 {
        vaddvq_u32(vandq_u32(v, w))
    }

    let h0 = vld1q_u32(ptr);
    let h1 = vld1q_u32(ptr.add(0x04));
    let h2 = vld1q_u32(ptr.add(0x08));
    let h3 = vld1q_u32(ptr.add(0x0C));
    let h4 = vld1q_u32(ptr.add(0x10));

    let m0 = mask4_fast(vceqq_u32(h0, v_target), weights);
    let m1 = mask4_fast(vceqq_u32(h1, v_target), weights);
    let m2 = mask4_fast(vceqq_u32(h2, v_target), weights);
    let m3 = mask4_fast(vceqq_u32(h3, v_target), weights);
    let m4 = mask4_fast(vceqq_u32(h4, v_target), weights);
    let mut match_mask = m0 | (m1 << 4) | (m2 << 8) | (m3 << 12) | (m4 << 16);

    let m20 = ((hashes[20] == target) as u32) << 20;
    match_mask |= m20;

    match_mask
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
pub(crate) unsafe fn probe_full_neon(hashes: &[u32; BUCKET_ENTRIES], target: u32) -> SimdProbeMask {
    use core::arch::aarch64::*;

    let ptr = hashes.as_ptr();

    let v_target = vdupq_n_u32(target);
    let v_empty = vdupq_n_u32(HASH_EMPTY);
    let v_tomb = vdupq_n_u32(HASH_TOMBSTONE);
    let weights = vld1q_u32([0x01, 0x02, 0x04, 0x08].as_ptr());

    #[inline(always)]
    unsafe fn mask4_fast(v: uint32x4_t, w: uint32x4_t) -> u32 {
        vaddvq_u32(vandq_u32(v, w))
    }

    let h0 = vld1q_u32(ptr);
    let h1 = vld1q_u32(ptr.add(0x04));
    let h2 = vld1q_u32(ptr.add(0x08));
    let h3 = vld1q_u32(ptr.add(0x0C));
    let h4 = vld1q_u32(ptr.add(0x10));

    let m0 = mask4_fast(vceqq_u32(h0, v_target), weights);
    let m1 = mask4_fast(vceqq_u32(h1, v_target), weights);
    let m2 = mask4_fast(vceqq_u32(h2, v_target), weights);
    let m3 = mask4_fast(vceqq_u32(h3, v_target), weights);
    let m4 = mask4_fast(vceqq_u32(h4, v_target), weights);
    let mut match_mask = m0 | (m1 << 4) | (m2 << 8) | (m3 << 12) | (m4 << 16);

    let e0 = mask4_fast(vceqq_u32(h0, v_empty), weights);
    let e1 = mask4_fast(vceqq_u32(h1, v_empty), weights);
    let e2 = mask4_fast(vceqq_u32(h2, v_empty), weights);
    let e3 = mask4_fast(vceqq_u32(h3, v_empty), weights);
    let e4 = mask4_fast(vceqq_u32(h4, v_empty), weights);
    let mut empty_mask = e0 | (e1 << 4) | (e2 << 8) | (e3 << 12) | (e4 << 16);

    let t0 = mask4_fast(vceqq_u32(h0, v_tomb), weights);
    let t1 = mask4_fast(vceqq_u32(h1, v_tomb), weights);
    let t2 = mask4_fast(vceqq_u32(h2, v_tomb), weights);
    let t3 = mask4_fast(vceqq_u32(h3, v_tomb), weights);
    let t4 = mask4_fast(vceqq_u32(h4, v_tomb), weights);
    let mut tomb_mask = t0 | (t1 << 4) | (t2 << 8) | (t3 << 12) | (t4 << 16);

    let m20 = ((hashes[20] == target) as u32) << 20;
    match_mask |= m20;

    let e20 = ((hashes[20] == HASH_EMPTY) as u32) << 20;
    empty_mask |= e20;

    let t20 = ((hashes[20] == HASH_TOMBSTONE) as u32) << 20;
    tomb_mask |= t20;

    SimdProbeMask { match_mask, empty_mask, tomb_mask }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
#[allow(dead_code)]
pub(crate) unsafe fn open_address_neon(
    hashes: &[u32; BUCKET_ENTRIES],
    target: u32,
) -> SimdProbeMask {
    probe_full_neon(hashes, target)
}

#[allow(dead_code)]
pub(crate) fn select_probe_match_kernel() -> ProbeMatchFn {
    #[cfg(target_arch = "x86_64")]
    {
        match frozen_core::isa::detect_best_isa_extension() {
            frozen_core::isa::ISAExtension::AVX2 => probe_match_avx2,
            frozen_core::isa::ISAExtension::SSE4_2
            | frozen_core::isa::ISAExtension::SSSE3
            | frozen_core::isa::ISAExtension::SSE2 => probe_match_sse2,
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        probe_match_neon
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        probe_match_scalar
    }
}

pub(crate) fn select_probe_full_kernel() -> ProbeFullFn {
    #[cfg(target_arch = "x86_64")]
    {
        match frozen_core::isa::detect_best_isa_extension() {
            frozen_core::isa::ISAExtension::AVX2 => probe_full_avx2,
            frozen_core::isa::ISAExtension::SSE4_2
            | frozen_core::isa::ISAExtension::SSSE3
            | frozen_core::isa::ISAExtension::SSE2 => probe_full_sse2,
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        probe_full_neon
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        probe_full_scalar
    }
}

#[allow(dead_code)]
pub(crate) fn select_probe_kernel() -> ProbeKernelFn {
    select_probe_full_kernel()
}

#[allow(dead_code)]
static PROBE_MATCH_KERNEL: std::sync::OnceLock<ProbeMatchFn> = std::sync::OnceLock::new();
static PROBE_FULL_KERNEL: std::sync::OnceLock<ProbeFullFn> = std::sync::OnceLock::new();

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn probe_match_kernel() -> ProbeMatchFn {
    *PROBE_MATCH_KERNEL.get_or_init(select_probe_match_kernel)
}

#[inline(always)]
pub(crate) fn probe_full_kernel() -> ProbeFullFn {
    *PROBE_FULL_KERNEL.get_or_init(select_probe_full_kernel)
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn probe_kernel() -> ProbeKernelFn {
    probe_full_kernel()
}

#[inline(always)]
pub(crate) fn probe_match(hashes: &[u32; BUCKET_ENTRIES], target: u32) -> u32 {
    #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
    {
        unsafe { probe_match_avx2(hashes, target) }
    }

    #[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
    {
        unsafe { probe_match_neon(hashes, target) }
    }

    #[cfg(not(any(
        all(target_arch = "x86_64", target_feature = "avx2"),
        all(target_arch = "aarch64", target_feature = "neon")
    )))]
    {
        let kernel = probe_match_kernel();
        unsafe { kernel(hashes, target) }
    }
}

#[inline(always)]
pub(crate) fn probe_full(hashes: &[u32; BUCKET_ENTRIES], target: u32) -> SimdProbeMask {
    #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
    {
        unsafe { probe_full_avx2(hashes, target) }
    }

    #[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
    {
        unsafe { probe_full_neon(hashes, target) }
    }

    #[cfg(not(any(
        all(target_arch = "x86_64", target_feature = "avx2"),
        all(target_arch = "aarch64", target_feature = "neon")
    )))]
    {
        let kernel = probe_full_kernel();
        unsafe { kernel(hashes, target) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_ENTRIES_MASK: u32 = (1 << BUCKET_ENTRIES) - 1;

    #[test]
    fn ok_scalar_empty_all() {
        let hashes = [HASH_EMPTY; BUCKET_ENTRIES];
        let mask = unsafe { probe_full_scalar(&hashes, 0x5442_4658) };

        assert_eq!(mask.match_mask, 0x00);
        assert_eq!(mask.empty_mask, ALL_ENTRIES_MASK);
        assert_eq!(mask.tomb_mask, 0x00);
    }

    #[test]
    fn ok_scalar_tombstones_all() {
        let hashes = [HASH_TOMBSTONE; BUCKET_ENTRIES];
        let mask = unsafe { probe_full_scalar(&hashes, 0x5442_4658) };

        assert_eq!(mask.match_mask, 0x00);
        assert_eq!(mask.empty_mask, 0x00);
        assert_eq!(mask.tomb_mask, ALL_ENTRIES_MASK);
    }

    #[test]
    fn ok_scalar_matches_and_entries() {
        let mut hashes = [HASH_EMPTY; BUCKET_ENTRIES];
        hashes[0] = 0x1234;
        hashes[1] = HASH_TOMBSTONE;
        hashes[5] = 0x1234;
        hashes[8] = 0x1234;
        hashes[9] = 0x5678;
        hashes[20] = 0x1234;

        let mask = unsafe { probe_full_scalar(&hashes, 0x1234) };
        assert_eq!(mask.match_mask, (1 << 0) | (1 << 5) | (1 << 8) | (1 << 20));
        assert_eq!(mask.tomb_mask, 1 << 1);

        let m_only = unsafe { probe_match_scalar(&hashes, 0x1234) };
        assert_eq!(m_only, (1 << 0) | (1 << 5) | (1 << 8) | (1 << 20));
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn ok_sse2_equivalence_with_scalar() {
        let target = 0x1234_5678;
        let mut t1 = [0x02; BUCKET_ENTRIES];
        t1[20] = target;
        let mut t2 = [HASH_EMPTY; BUCKET_ENTRIES];
        t2[19] = target;
        t2[20] = HASH_TOMBSTONE;
        let test_cases = [t1, [HASH_EMPTY; BUCKET_ENTRIES], [HASH_TOMBSTONE; BUCKET_ENTRIES], t2];

        for hashes in &test_cases {
            let scalar_res = unsafe { probe_full_scalar(hashes, target) };
            let sse2_res = unsafe { probe_full_sse2(hashes, target) };
            assert_eq!(scalar_res, sse2_res);

            let scalar_m = unsafe { probe_match_scalar(hashes, target) };
            let sse2_m = unsafe { probe_match_sse2(hashes, target) };
            assert_eq!(scalar_m, sse2_m);
        }
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn ok_avx2_equivalence_with_scalar() {
        use frozen_core::isa::{ISAExtension, detect_best_isa_extension};

        if detect_best_isa_extension() != ISAExtension::AVX2 {
            return;
        }

        let target = 0x9ABC_DEF0;
        let mut t1 = [0x02; BUCKET_ENTRIES];
        t1[20] = target;
        let mut t2 = [HASH_EMPTY; BUCKET_ENTRIES];
        t2[16] = target;
        t2[20] = HASH_TOMBSTONE;
        let test_cases = [t1, [HASH_EMPTY; BUCKET_ENTRIES], [HASH_TOMBSTONE; BUCKET_ENTRIES], t2];

        for hashes in &test_cases {
            let scalar_res = unsafe { probe_full_scalar(hashes, target) };
            let avx2_res = unsafe { probe_full_avx2(hashes, target) };
            assert_eq!(scalar_res, avx2_res);

            let scalar_m = unsafe { probe_match_scalar(hashes, target) };
            let avx2_m = unsafe { probe_match_avx2(hashes, target) };
            assert_eq!(scalar_m, avx2_m);
        }
    }

    #[test]
    #[cfg(target_arch = "aarch64")]
    fn ok_neon_equivalence_with_scalar() {
        let target = 0x8765_4321;
        let mut t1 = [0x02; BUCKET_ENTRIES];
        t1[20] = target;
        let test_cases = [t1, [HASH_EMPTY; BUCKET_ENTRIES], [HASH_TOMBSTONE; BUCKET_ENTRIES]];

        for hashes in &test_cases {
            let scalar_res = unsafe { probe_full_scalar(hashes, target) };
            let neon_res = unsafe { probe_full_neon(hashes, target) };
            assert_eq!(scalar_res, neon_res);

            let scalar_m = unsafe { probe_match_scalar(hashes, target) };
            let neon_m = unsafe { probe_match_neon(hashes, target) };
            assert_eq!(scalar_m, neon_m);
        }
    }

    #[test]
    fn ok_kernel_consistency() {
        let mut hashes = [HASH_EMPTY; BUCKET_ENTRIES];
        hashes[2] = 0xAA;
        hashes[8] = 0xAA;
        hashes[20] = 0xAA;
        hashes[9] = HASH_TOMBSTONE;

        let mask = probe_full(&hashes, 0xAA);
        assert_eq!(mask.match_mask, (1 << 2) | (1 << 8) | (1 << 20));
        assert_eq!(mask.tomb_mask, 1 << 9);

        let match_mask = probe_match(&hashes, 0xAA);
        assert_eq!(match_mask, (1 << 2) | (1 << 8) | (1 << 20));
    }
}
