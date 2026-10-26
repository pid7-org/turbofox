use crate::common::{HASH_EMPTY, HASH_TOMBSTONE};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SimdProbeMask {
    pub(crate) match_mask: u16,
    pub(crate) empty_mask: u16,
    pub(crate) tomb_mask: u16,
}

pub(crate) type ProbeKernelFn = unsafe fn(&[u32; 0x10], u32) -> SimdProbeMask;

#[inline(always)]
#[allow(dead_code)]
pub(crate) unsafe fn open_address_scalar(hashes: &[u32; 0x10], target: u32) -> SimdProbeMask {
    let mut match_mask = 0u16;
    let mut empty_mask = 0u16;
    let mut tomb_mask = 0u16;

    let mut i = 0x00;
    while i < 0x10 {
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

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
pub(crate) unsafe fn open_address_sse2(hashes: &[u32; 0x10], target: u32) -> SimdProbeMask {
    use core::arch::x86_64::*;

    let ptr = hashes.as_ptr() as *const __m128i;

    let v_empty = _mm_setzero_si128();
    let v_target = _mm_set1_epi32(target as i32);
    let v_tomb = _mm_set1_epi32(HASH_TOMBSTONE as i32);

    let h0 = _mm_loadu_si128(ptr);
    let h1 = _mm_loadu_si128(ptr.add(0x01));
    let h2 = _mm_loadu_si128(ptr.add(0x02));
    let h3 = _mm_loadu_si128(ptr.add(0x03));

    let m0 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h0, v_target)));
    let m1 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h1, v_target)));
    let m2 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h2, v_target)));
    let m3 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h3, v_target)));
    let match_mask = (m0 | (m1 << 4) | (m2 << 8) | (m3 << 0x0C)) as u16;

    let e0 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h0, v_empty)));
    let e1 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h1, v_empty)));
    let e2 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h2, v_empty)));
    let e3 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h3, v_empty)));
    let empty_mask = (e0 | (e1 << 4) | (e2 << 8) | (e3 << 0x0C)) as u16;

    let t0 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h0, v_tomb)));
    let t1 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h1, v_tomb)));
    let t2 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h2, v_tomb)));
    let t3 = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpeq_epi32(h3, v_tomb)));
    let tomb_mask = (t0 | (t1 << 4) | (t2 << 8) | (t3 << 0x0C)) as u16;

    SimdProbeMask { match_mask, empty_mask, tomb_mask }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn open_address_avx2(hashes: &[u32; 0x10], target: u32) -> SimdProbeMask {
    use core::arch::x86_64::*;

    let ptr = hashes.as_ptr() as *const __m256i;

    let v_empty = _mm256_setzero_si256();
    let v_target = _mm256_set1_epi32(target as i32);
    let v_tomb = _mm256_set1_epi32(HASH_TOMBSTONE as i32);

    let h0 = _mm256_loadu_si256(ptr);
    let h1 = _mm256_loadu_si256(ptr.add(0x01));

    let m0 = _mm256_movemask_ps(_mm256_castsi256_ps(_mm256_cmpeq_epi32(h0, v_target)));
    let m1 = _mm256_movemask_ps(_mm256_castsi256_ps(_mm256_cmpeq_epi32(h1, v_target)));
    let match_mask = (m0 | (m1 << 8)) as u16;

    let e0 = _mm256_movemask_ps(_mm256_castsi256_ps(_mm256_cmpeq_epi32(h0, v_empty)));
    let e1 = _mm256_movemask_ps(_mm256_castsi256_ps(_mm256_cmpeq_epi32(h1, v_empty)));
    let empty_mask = (e0 | (e1 << 8)) as u16;

    let t0 = _mm256_movemask_ps(_mm256_castsi256_ps(_mm256_cmpeq_epi32(h0, v_tomb)));
    let t1 = _mm256_movemask_ps(_mm256_castsi256_ps(_mm256_cmpeq_epi32(h1, v_tomb)));
    let tomb_mask = (t0 | (t1 << 8)) as u16;

    SimdProbeMask { match_mask, empty_mask, tomb_mask }
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
pub(crate) unsafe fn open_address_neon(hashes: &[u32; 0x10], target: u32) -> SimdProbeMask {
    use core::arch::aarch64::*;

    let ptr = hashes.as_ptr();

    let v_target = vdupq_n_u32(target);
    let v_empty = vdupq_n_u32(HASH_EMPTY);
    let v_tomb = vdupq_n_u32(HASH_TOMBSTONE);

    let h0 = vld1q_u32(ptr);
    let h1 = vld1q_u32(ptr.add(0x04));
    let h2 = vld1q_u32(ptr.add(0x08));
    let h3 = vld1q_u32(ptr.add(0x0C));

    #[inline(always)]
    unsafe fn mask4(v: uint32x4_t) -> u16 {
        let b0 = (vgetq_lane_u32::<0>(v) & 1) as u16;
        let b1 = ((vgetq_lane_u32::<1>(v) & 1) as u16) << 1;
        let b2 = ((vgetq_lane_u32::<2>(v) & 1) as u16) << 2;
        let b3 = ((vgetq_lane_u32::<3>(v) & 1) as u16) << 3;

        b0 | b1 | b2 | b3
    }

    let m0 = mask4(vceqq_u32(h0, v_target));
    let m1 = mask4(vceqq_u32(h1, v_target));
    let m2 = mask4(vceqq_u32(h2, v_target));
    let m3 = mask4(vceqq_u32(h3, v_target));
    let match_mask = m0 | (m1 << 4) | (m2 << 8) | (m3 << 0x0C);

    let e0 = mask4(vceqq_u32(h0, v_empty));
    let e1 = mask4(vceqq_u32(h1, v_empty));
    let e2 = mask4(vceqq_u32(h2, v_empty));
    let e3 = mask4(vceqq_u32(h3, v_empty));
    let empty_mask = e0 | (e1 << 4) | (e2 << 8) | (e3 << 0x0C);

    let t0 = mask4(vceqq_u32(h0, v_tomb));
    let t1 = mask4(vceqq_u32(h1, v_tomb));
    let t2 = mask4(vceqq_u32(h2, v_tomb));
    let t3 = mask4(vceqq_u32(h3, v_tomb));
    let tomb_mask = t0 | (t1 << 4) | (t2 << 8) | (t3 << 0x0C);

    SimdProbeMask { match_mask, empty_mask, tomb_mask }
}

pub(crate) fn select_probe_kernel() -> ProbeKernelFn {
    #[cfg(target_arch = "x86_64")]
    {
        match frozen_core::isa::detect_best_isa_extension() {
            frozen_core::isa::ISAExtension::AVX2 => open_address_avx2,
            frozen_core::isa::ISAExtension::SSE4_2
            | frozen_core::isa::ISAExtension::SSSE3
            | frozen_core::isa::ISAExtension::SSE2 => open_address_sse2,
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        open_address_neon
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        open_address_scalar
    }
}

static PROBE_KERNEL: std::sync::OnceLock<ProbeKernelFn> = std::sync::OnceLock::new();

#[inline(always)]
pub(crate) fn probe_kernel() -> ProbeKernelFn {
    *PROBE_KERNEL.get_or_init(select_probe_kernel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_scalar_empty_all() {
        let hashes = [HASH_EMPTY; 0x10];
        let mask = unsafe { open_address_scalar(&hashes, 0x5442_4658) };

        assert_eq!(mask.match_mask, 0x00);
        assert_eq!(mask.empty_mask, 0xFFFF);
        assert_eq!(mask.tomb_mask, 0x00);
    }

    #[test]
    fn ok_scalar_tombstones_all() {
        let hashes = [HASH_TOMBSTONE; 0x10];
        let mask = unsafe { open_address_scalar(&hashes, 0x5442_4658) };

        assert_eq!(mask.match_mask, 0x00);
        assert_eq!(mask.empty_mask, 0x00);
        assert_eq!(mask.tomb_mask, 0xFFFF);
    }

    #[test]
    fn ok_scalar_matches_positions() {
        let target = 0x5442_4658;
        let mut hashes = [0x02; 0x10];

        for i in 0x00..0x10 {
            hashes[i] = target;

            let mask = unsafe { open_address_scalar(&hashes, target) };
            assert_eq!(mask.match_mask, 1 << i);

            hashes[i] = 0x02;
        }
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn ok_sse2_equivalence_with_scalar() {
        let target = 0x1234_5678;
        let test_cases = [
            [0x02; 0x10],
            [HASH_EMPTY; 0x10],
            [HASH_TOMBSTONE; 0x10],
            [
                target,
                HASH_EMPTY,
                HASH_TOMBSTONE,
                0x02,
                0x05,
                target,
                HASH_EMPTY,
                0x99,
                HASH_TOMBSTONE,
                target,
                0x00,
                0x01,
                target,
                target,
                0x77,
                target,
            ],
        ];

        for hashes in &test_cases {
            let scalar_res = unsafe { open_address_scalar(hashes, target) };
            let sse2_res = unsafe { open_address_sse2(hashes, target) };

            assert_eq!(scalar_res, sse2_res);
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
        let test_cases = [
            [0x02; 0x10],
            [HASH_EMPTY; 0x10],
            [HASH_TOMBSTONE; 0x10],
            [
                target,
                HASH_EMPTY,
                HASH_TOMBSTONE,
                0x02,
                0x05,
                target,
                HASH_EMPTY,
                0x99,
                HASH_TOMBSTONE,
                target,
                0x00,
                0x01,
                target,
                target,
                0x77,
                target,
            ],
            [
                0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F,
                target, 0x01, 0x00,
            ],
        ];

        for hashes in &test_cases {
            let scalar_res = unsafe { open_address_scalar(hashes, target) };
            let avx2_res = unsafe { open_address_avx2(hashes, target) };

            assert_eq!(scalar_res, avx2_res);
        }
    }

    #[test]
    #[cfg(target_arch = "aarch64")]
    fn ok_neon_equivalence_with_scalar() {
        let target = 0x8765_4321;
        let test_cases = [
            [0x02; 0x10],
            [HASH_EMPTY; 0x10],
            [HASH_TOMBSTONE; 0x10],
            [
                target,
                HASH_EMPTY,
                HASH_TOMBSTONE,
                0x02,
                0x05,
                target,
                HASH_EMPTY,
                0x99,
                HASH_TOMBSTONE,
                target,
                0x00,
                0x01,
                target,
                target,
                0x77,
                target,
            ],
        ];

        for hashes in &test_cases {
            let scalar_res = unsafe { open_address_scalar(hashes, target) };
            let neon_res = unsafe { open_address_neon(hashes, target) };

            assert_eq!(scalar_res, neon_res);
        }
    }

    #[test]
    fn ok_runtime_probe_kernel_selection() {
        let kernel = probe_kernel();
        let target = 0xCAFE_BABE;

        let mut hashes = [0x02; 0x10];
        hashes[0x05] = target;
        hashes[0x09] = HASH_EMPTY;
        hashes[0x0D] = HASH_TOMBSTONE;

        let mask = unsafe { kernel(&hashes, target) };
        assert_eq!(mask.match_mask, 1 << 0x05);
        assert_eq!(mask.empty_mask, 1 << 0x09);
        assert_eq!(mask.tomb_mask, 1 << 0x0D);
    }
}
