pub(crate) const FP_EMPTY: u16 = 0x0000;
pub(crate) const FP_TOMBSTONE: u16 = 0x0001;
pub(crate) const MIN_VALID_FP: u16 = 0x0002;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FpProbeMask {
    pub(crate) match_mask: u16,
    pub(crate) empty_mask: u16,
    pub(crate) tomb_mask: u16,
}

pub(crate) type IndexProbeKernelFn = unsafe fn(&[u16; 0x10], u16) -> FpProbeMask;

#[allow(dead_code)]
#[inline(always)]
pub(crate) unsafe fn index_probe_scalar(fps: &[u16; 0x10], target: u16) -> FpProbeMask {
    let mut match_mask = 0u16;
    let mut empty_mask = 0u16;
    let mut tomb_mask = 0u16;

    let mut i = 0x00;
    while i < 0x10 {
        let fp = fps[i];
        if fp == target {
            match_mask |= 1 << i;
        }

        if fp == FP_EMPTY {
            empty_mask |= 1 << i;
        } else if fp == FP_TOMBSTONE {
            tomb_mask |= 1 << i;
        }

        i += 0x01;
    }

    FpProbeMask { match_mask, empty_mask, tomb_mask }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
pub(crate) unsafe fn index_probe_sse2(fps: &[u16; 0x10], target: u16) -> FpProbeMask {
    use core::arch::x86_64::*;

    let ptr = fps.as_ptr() as *const __m128i;

    let v_empty = _mm_setzero_si128();
    let v_target = _mm_set1_epi16(target as i16);
    let v_tomb = _mm_set1_epi16(FP_TOMBSTONE as i16);

    let h0 = _mm_loadu_si128(ptr);
    let h1 = _mm_loadu_si128(ptr.add(0x01));

    let c0 = _mm_cmpeq_epi16(h0, v_target);
    let c1 = _mm_cmpeq_epi16(h1, v_target);
    let match_mask = _mm_movemask_epi8(_mm_packs_epi16(c0, c1)) as u16;

    let e0 = _mm_cmpeq_epi16(h0, v_empty);
    let e1 = _mm_cmpeq_epi16(h1, v_empty);
    let empty_mask = _mm_movemask_epi8(_mm_packs_epi16(e0, e1)) as u16;

    let t0 = _mm_cmpeq_epi16(h0, v_tomb);
    let t1 = _mm_cmpeq_epi16(h1, v_tomb);
    let tomb_mask = _mm_movemask_epi8(_mm_packs_epi16(t0, t1)) as u16;

    FpProbeMask { match_mask, empty_mask, tomb_mask }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub(crate) unsafe fn index_probe_avx2(fps: &[u16; 0x10], target: u16) -> FpProbeMask {
    use core::arch::x86_64::*;

    let ptr = fps.as_ptr() as *const __m256i;

    let v_empty = _mm256_setzero_si256();
    let v_target = _mm256_set1_epi16(target as i16);
    let v_tomb = _mm256_set1_epi16(FP_TOMBSTONE as i16);

    let h = _mm256_loadu_si256(ptr);

    let c = _mm256_cmpeq_epi16(h, v_target);
    let lo_c = _mm256_castsi256_si128(c);
    let hi_c = _mm256_extracti128_si256(c, 0x01);
    let match_mask = _mm_movemask_epi8(_mm_packs_epi16(lo_c, hi_c)) as u16;

    let e = _mm256_cmpeq_epi16(h, v_empty);
    let lo_e = _mm256_castsi256_si128(e);
    let hi_e = _mm256_extracti128_si256(e, 0x01);
    let empty_mask = _mm_movemask_epi8(_mm_packs_epi16(lo_e, hi_e)) as u16;

    let t = _mm256_cmpeq_epi16(h, v_tomb);
    let lo_t = _mm256_castsi256_si128(t);
    let hi_t = _mm256_extracti128_si256(t, 0x01);
    let tomb_mask = _mm_movemask_epi8(_mm_packs_epi16(lo_t, hi_t)) as u16;

    FpProbeMask { match_mask, empty_mask, tomb_mask }
}

#[cfg(target_arch = "aarch64")]
pub(crate) unsafe fn index_probe_neon(fps: &[u16; 0x10], target: u16) -> FpProbeMask {
    use core::arch::aarch64::*;

    let ptr = fps.as_ptr();

    let v_target = vdupq_n_u16(target);
    let v_empty = vdupq_n_u16(FP_EMPTY);
    let v_tomb = vdupq_n_u16(FP_TOMBSTONE);

    let h0 = vld1q_u16(ptr);
    let h1 = vld1q_u16(ptr.add(0x08));

    #[inline(always)]
    unsafe fn mask8(v: uint16x8_t) -> u16 {
        let b0 = vgetq_lane_u16::<0>(v) & 1;
        let b1 = (vgetq_lane_u16::<1>(v) & 1) << 1;
        let b2 = (vgetq_lane_u16::<2>(v) & 1) << 2;
        let b3 = (vgetq_lane_u16::<3>(v) & 1) << 3;
        let b4 = (vgetq_lane_u16::<4>(v) & 1) << 4;
        let b5 = (vgetq_lane_u16::<5>(v) & 1) << 5;
        let b6 = (vgetq_lane_u16::<6>(v) & 1) << 6;
        let b7 = (vgetq_lane_u16::<7>(v) & 1) << 7;

        b0 | b1 | b2 | b3 | b4 | b5 | b6 | b7
    }

    let m0 = mask8(vceqq_u16(h0, v_target));
    let m1 = mask8(vceqq_u16(h1, v_target));
    let match_mask = m0 | (m1 << 0x08);

    let e0 = mask8(vceqq_u16(h0, v_empty));
    let e1 = mask8(vceqq_u16(h1, v_empty));
    let empty_mask = e0 | (e1 << 0x08);

    let t0 = mask8(vceqq_u16(h0, v_tomb));
    let t1 = mask8(vceqq_u16(h1, v_tomb));
    let tomb_mask = t0 | (t1 << 0x08);

    FpProbeMask { match_mask, empty_mask, tomb_mask }
}

pub(crate) fn select_index_probe_kernel() -> IndexProbeKernelFn {
    #[cfg(target_arch = "x86_64")]
    {
        match frozen_core::isa::detect_best_isa_extension() {
            frozen_core::isa::ISAExtension::AVX2 => index_probe_avx2,
            frozen_core::isa::ISAExtension::SSE4_2
            | frozen_core::isa::ISAExtension::SSSE3
            | frozen_core::isa::ISAExtension::SSE2 => index_probe_sse2,
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        index_probe_neon
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        index_probe_scalar
    }
}

static INDEX_PROBE_KERNEL: std::sync::OnceLock<IndexProbeKernelFn> = std::sync::OnceLock::new();

#[inline(always)]
pub(crate) fn index_probe_kernel() -> IndexProbeKernelFn {
    *INDEX_PROBE_KERNEL.get_or_init(select_index_probe_kernel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_scalar_empty_all() {
        let fps = [FP_EMPTY; 0x10];
        let mask = unsafe { index_probe_scalar(&fps, 0x5442) };

        assert_eq!(mask.match_mask, 0x00);
        assert_eq!(mask.empty_mask, 0xFFFF);
        assert_eq!(mask.tomb_mask, 0x00);
    }

    #[test]
    fn ok_scalar_tombstones_all() {
        let fps = [FP_TOMBSTONE; 0x10];
        let mask = unsafe { index_probe_scalar(&fps, 0x5442) };

        assert_eq!(mask.match_mask, 0x00);
        assert_eq!(mask.empty_mask, 0x00);
        assert_eq!(mask.tomb_mask, 0xFFFF);
    }

    #[test]
    fn ok_scalar_matches_positions() {
        let mut fps = [0x02; 0x10];
        fps[0x00] = 0x1234;
        fps[0x05] = 0x1234;
        fps[0x0F] = 0x1234;
        fps[0x03] = FP_EMPTY;
        fps[0x07] = FP_TOMBSTONE;

        let mask = unsafe { index_probe_scalar(&fps, 0x1234) };
        assert_eq!(mask.match_mask, (1 << 0x00) | (1 << 0x05) | (1 << 0x0F));
        assert_eq!(mask.empty_mask, 1 << 0x03);
        assert_eq!(mask.tomb_mask, 1 << 0x07);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn ok_sse2_equivalence_with_scalar() {
        let patterns: [[u16; 0x10]; 4] = [
            [FP_EMPTY; 0x10],
            [FP_TOMBSTONE; 0x10],
            [0xABCD; 0x10],
            [
                0x0002,
                0xABCD,
                FP_EMPTY,
                0xABCD,
                0x1111,
                FP_TOMBSTONE,
                0x2222,
                0xABCD,
                FP_EMPTY,
                0x3333,
                0x4444,
                FP_TOMBSTONE,
                0x5555,
                0x6666,
                0x7777,
                0xABCD,
            ],
        ];

        let target = 0xABCD;
        for fps in &patterns {
            let scalar_res = unsafe { index_probe_scalar(fps, target) };
            let sse2_res = unsafe { index_probe_sse2(fps, target) };

            assert_eq!(scalar_res, sse2_res);
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn ok_avx2_equivalence_with_scalar() {
        if !is_x86_feature_detected!("avx2") {
            return;
        }

        let patterns: [[u16; 0x10]; 4] = [
            [FP_EMPTY; 0x10],
            [FP_TOMBSTONE; 0x10],
            [0xBEEF; 0x10],
            [
                0x0002,
                0xBEEF,
                FP_EMPTY,
                0xBEEF,
                0x1111,
                FP_TOMBSTONE,
                0x2222,
                0xBEEF,
                FP_EMPTY,
                0x3333,
                0x4444,
                FP_TOMBSTONE,
                0x5555,
                0x6666,
                0x7777,
                0xBEEF,
            ],
        ];

        let target = 0xBEEF;
        for fps in &patterns {
            let scalar_res = unsafe { index_probe_scalar(fps, target) };
            let avx2_res = unsafe { index_probe_avx2(fps, target) };

            assert_eq!(scalar_res, avx2_res);
        }
    }

    #[test]
    fn ok_runtime_index_probe_kernel_selection() {
        let kernel = index_probe_kernel();
        let target = 0xCAFE;

        let mut fps = [0x02; 0x10];
        fps[0x03] = target;
        fps[0x08] = FP_EMPTY;
        fps[0x0C] = FP_TOMBSTONE;

        let mask = unsafe { kernel(&fps, target) };
        assert_eq!(mask.match_mask, 1 << 0x03);
        assert_eq!(mask.empty_mask, 1 << 0x08);
        assert_eq!(mask.tomb_mask, 1 << 0x0C);
    }
}
