//! ADR-0063 D1 — Despachante Hamming: scalar | AVX2 XOR | AVX-512.
//! Runtime adaptive: `#[target_feature]` permite compilar kernels SIMD mesmo em
//! build soft-float. `select_best_hamming_kernel()` escolhe baseado na CPU.
//!
//! Seam: `cpu_caps()` — em `std` detecta via `is_x86_feature_detected!`; em
//! `no_std` retorna caps conservadoras (scalar) salvo `set_cpu_caps()` chamada
//! pelo embedder.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

const PATH_SCALAR: u8 = 0;
const PATH_AVX2_XOR: u8 = 1;
const PATH_AVX512: u8 = 2;

static HAMMING_PATH: AtomicU8 = AtomicU8::new(PATH_SCALAR);
static SELECTED: AtomicBool = AtomicBool::new(false);
static MANUAL_CAPS: AtomicU8 = AtomicU8::new(0); // bit0=avx2, bit1=avx512

/// Capacidades SIMD da CPU (runtime).
#[derive(Clone, Copy, Debug, Default)]
pub struct CpuCaps {
    pub avx2: bool,
    pub avx512: bool,
}

/// Detecta/consulta capacidades SIMD. Com `simd-runtime` (default), auto-detecta
/// via `is_x86_feature_detected!`; sem ele, usa o que foi injetado via
/// `set_cpu_caps` (default: scalar — sempre correto).
#[cfg(all(feature = "simd-runtime", target_arch = "x86_64"))]
pub fn cpu_caps() -> CpuCaps {
    CpuCaps {
        avx2: std::arch::is_x86_feature_detected!("avx2"),
        avx512: std::arch::is_x86_feature_detected!("avx512f"),
    }
}

#[cfg(not(all(feature = "simd-runtime", target_arch = "x86_64")))]
pub fn cpu_caps() -> CpuCaps {
    let v = MANUAL_CAPS.load(Ordering::Relaxed);
    CpuCaps {
        avx2: v & 1 != 0,
        avx512: v & 2 != 0,
    }
}

/// Injeta capacidades (no_std: chamar no boot do embedder).
/// Rearma `SELECTED` para que a próxima chamada reavalie o kernel — sem isso,
/// um `hamming()` antes da injeção travaria o path em scalar para sempre
/// (bughunt #9).
pub fn set_cpu_caps(c: CpuCaps) {
    let mut v = 0u8;
    if c.avx2 {
        v |= 1;
    }
    if c.avx512 {
        v |= 2;
    }
    MANUAL_CAPS.store(v, Ordering::Relaxed);
    SELECTED.store(false, Ordering::Relaxed);
}

pub fn cpu_has_avx2() -> bool {
    cpu_caps().avx2
}

pub fn cpu_has_avx512() -> bool {
    cpu_caps().avx512
}

pub type HammingFn = fn(&[u64], &[u64]) -> u32;

/// Escolhe o melhor kernel disponível (idempotente, lazy).
pub fn select_best_hamming_kernel() {
    let caps = cpu_caps();
    if caps.avx512 {
        HAMMING_PATH.store(PATH_AVX512, Ordering::Relaxed);
    } else if caps.avx2 {
        HAMMING_PATH.store(PATH_AVX2_XOR, Ordering::Relaxed);
    } else {
        HAMMING_PATH.store(PATH_SCALAR, Ordering::Relaxed);
    }
}

#[inline(always)]
fn ensure_selected() {
    // load+store em vez de swap(true): o swap é um RMW com `lock` no x86 —
    // medido dominando hamming() de vetores curtos (loop de N vetores no
    // top_k). Corrida benigna: dois selects concorrentes dão o mesmo resultado
    // (idempotente), e `set_cpu_caps` rearms SELECTED para reavaliar.
    if !SELECTED.load(Ordering::Relaxed) {
        SELECTED.store(true, Ordering::Relaxed);
        select_best_hamming_kernel();
    }
}

#[inline(always)]
pub fn path_name() -> &'static str {
    ensure_selected();
    match HAMMING_PATH.load(Ordering::Relaxed) {
        PATH_AVX512 => "avx512",
        PATH_AVX2_XOR => "avx2_xor",
        _ => "scalar",
    }
}

#[inline(always)]
pub fn active_kernel() -> HammingFn {
    ensure_selected();
    match HAMMING_PATH.load(Ordering::Relaxed) {
        PATH_AVX512 => hamming_avx512_or_fallback,
        PATH_AVX2_XOR => hamming_avx2_or_fallback,
        _ => hamming_scalar,
    }
}

#[inline(always)]
pub fn hamming(a: &[u64], b: &[u64]) -> u32 {
    active_kernel()(a, b)
}

/// 1024-dim = 16×u64 — caminho hot L4/L5.
#[inline(always)]
pub fn hamming_1024(v1: &[u64; 16], v2: &[u64; 16]) -> u32 {
    hamming(v1.as_slice(), v2.as_slice())
}

pub fn hamming_scalar(a: &[u64], b: &[u64]) -> u32 {
    let n = a.len().min(b.len());
    let mut d = 0u32;
    for i in 0..n {
        d += (a[i] ^ b[i]).count_ones();
    }
    let longer = if a.len() > b.len() { a } else { b };
    for &w in longer[n..].iter() {
        d += w.count_ones();
    }
    d
}

// ─── AVX2 kernel ──────────────────────────────────────────────
// Compilado via #[target_feature(enable = "avx2")] mesmo em build soft-float.
// Runtime: só chamado se cpu_has_avx2(). fallback → hamming_scalar.

#[cfg(target_arch = "x86_64")]
fn hamming_avx2_or_fallback(a: &[u64], b: &[u64]) -> u32 {
    if cpu_has_avx2() {
        // SAFETY: cpu_has_avx2() confirmou suporte runtime ao AVX2; o kernel só
        // executa instruções AVX2. a/b são slices válidos — o kernel nunca lê
        // além de a[..min(a.len,b.len)] e do tail longer[n..] (bounds por
        // construção, ver hamming_avx2_xor).
        return unsafe { hamming_avx2_xor(a, b) };
    }
    hamming_scalar(a, b)
}

/// AVX2: XOR YMM + popcount via GPR extract (sem VPSHUFB, sem store p/ mem).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
// SAFETY: requer AVX2 habilitado (target_feature) E suporte runtime — o
// chamador deve ter verificado cpu_has_avx2(). a/b precisam de pelo menos
// min(a.len,b.len) words legíveis (invariante de slice); leituras `loadu`
// unaligned no main loop só ocorrem com i+4 <= n (32 bytes inteiros dentro
// do slice); o tail loop e longer[n..] respeitam len().
unsafe fn hamming_avx2_xor(a: &[u64], b: &[u64]) -> u32 {
    use core::arch::x86_64::*;
    let n = a.len().min(b.len());
    let mut d = 0u32;
    let mut i = 0;
    while i + 4 <= n {
        let va = _mm256_loadu_si256(a.as_ptr().add(i) as *const __m256i);
        let vb = _mm256_loadu_si256(b.as_ptr().add(i) as *const __m256i);
        let x = _mm256_xor_si256(va, vb);
        let lo128 = _mm256_extracti128_si256::<0>(x);
        let hi128 = _mm256_extracti128_si256::<1>(x);
        d += (_mm_cvtsi128_si64(lo128) as u64).count_ones();
        d += (_mm_extract_epi64::<1>(lo128) as u64).count_ones();
        d += (_mm_cvtsi128_si64(hi128) as u64).count_ones();
        d += (_mm_extract_epi64::<1>(hi128) as u64).count_ones();
        i += 4;
    }
    while i < n {
        d += (a[i] ^ b[i]).count_ones();
        i += 1;
    }
    let longer = if a.len() > b.len() { a } else { b };
    for &w in longer[n..].iter() {
        d += w.count_ones();
    }
    d
}

// ─── AVX-512 kernels ──────────────────────────────────────────

#[cfg(target_arch = "x86_64")]
fn hamming_avx512_or_fallback(a: &[u64], b: &[u64]) -> u32 {
    if cpu_has_avx512() {
        // SAFETY: cpu_has_avx512() confirmou AVX-512F runtime; o dispatch
        // (via __cpuid_count) desce para um kernel cujo target_feature cobre
        // as instruções usadas. a/b são slices válidos (bounds internos).
        return unsafe { hamming_avx512_dispatch(a, b) };
    }
    if cpu_has_avx2() {
        // SAFETY: cpu_has_avx2() confirmou AVX2 runtime — mesmas garantias de
        // hamming_avx2_or_fallback acima.
        return unsafe { hamming_avx2_xor(a, b) };
    }
    hamming_scalar(a, b)
}

// SAFETY: requer AVX-512F habilitado no runtime (chamador verificou
// cpu_has_avx512()). Usa __cpuid_count (intrínseco, instrução CPUID) para
// decidir entre kernels AVX-512 — `has_vpop` é o bit 14 do leaf 7/ecx
// (AVX512_VPOPCNTDQ). a/b são slices válidos; bounds internos em cada kernel.
#[cfg(target_arch = "x86_64")]
unsafe fn hamming_avx512_dispatch(a: &[u64], b: &[u64]) -> u32 {
    let leaf7 = core::arch::x86_64::__cpuid_count(7, 0);
    let has_vpop = (leaf7.ecx & (1 << 14)) != 0;
    if has_vpop {
        hamming_avx512_vpopcnt(a, b)
    } else {
        hamming_avx512_xor(a, b)
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f", enable = "avx512vpopcntdq")]
// SAFETY: requer AVX-512F + AVX-512VPOPCNTDQ (target_feature) e suporte
// runtime — chamador verificou cpu_has_avx512() E o dispatch confirmou o bit
// vpopcnt via CPUID. a/b: slices válidos; o main loop usa loadu só com
// i+8 <= n (64 bytes inteiros dentro do slice); tail e longer[n..] por len().
unsafe fn hamming_avx512_vpopcnt(a: &[u64], b: &[u64]) -> u32 {
    use core::arch::x86_64::*;
    let n = a.len().min(b.len());
    let mut d = 0u32;
    let mut i = 0;
    while i + 8 <= n {
        let va = _mm512_loadu_si512(a.as_ptr().add(i) as *const _);
        let vb = _mm512_loadu_si512(b.as_ptr().add(i) as *const _);
        let x = _mm512_xor_si512(va, vb);
        let pc = _mm512_popcnt_epi64(x);
        let mut tmp = [0u64; 8];
        _mm512_storeu_si512(tmp.as_mut_ptr() as *mut _, pc);
        for t in &tmp {
            d += *t as u32;
        }
        i += 8;
    }
    while i < n {
        d += (a[i] ^ b[i]).count_ones();
        i += 1;
    }
    let longer = if a.len() > b.len() { a } else { b };
    for &w in longer[n..].iter() {
        d += w.count_ones();
    }
    d
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
// SAFETY: requer AVX-512F (target_feature) e suporte runtime — chamador
// verificou cpu_has_avx512() e o dispatch NÃO viu o bit vpopcnt (sem VPOPCNTDQ,
// usa XOR ZMM + popcount via store p/ mem local). a/b: slices válidos; loadu
// só com i+8 <= n; tail e longer[n..] por len().
unsafe fn hamming_avx512_xor(a: &[u64], b: &[u64]) -> u32 {
    use core::arch::x86_64::*;
    let n = a.len().min(b.len());
    let mut d = 0u32;
    let mut i = 0;
    while i + 8 <= n {
        let va = _mm512_loadu_si512(a.as_ptr().add(i) as *const _);
        let vb = _mm512_loadu_si512(b.as_ptr().add(i) as *const _);
        let x = _mm512_xor_si512(va, vb);
        let mut tmp = [0u64; 8];
        _mm512_storeu_si512(tmp.as_mut_ptr() as *mut _, x);
        for t in &tmp {
            d += t.count_ones();
        }
        i += 8;
    }
    while i < n {
        d += (a[i] ^ b[i]).count_ones();
        i += 1;
    }
    let longer = if a.len() > b.len() { a } else { b };
    for &w in longer[n..].iter() {
        d += w.count_ones();
    }
    d
}

/// Smoke: 1024-dim (16 words) top-1 idêntico scalar vs kernel ativo.
pub fn smoke_1024() -> bool {
    ensure_selected();
    let mut a = [0u64; 16];
    let mut b = [0u64; 16];
    let mut c = [0u64; 16];
    a[0] = 0xFFFF_FFFF_FFFF_FFFF;
    b[0] = 0xFFFF_FFFF_FFFF_FFFF; // dist 0
    c[0] = 0; // dist 64
    let d_ab = hamming_1024(&a, &b);
    let d_ac = hamming_1024(&a, &c);
    let d_s = hamming_scalar(&a, &c);
    d_ab == 0 && d_ac == 64 && d_ac == d_s
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn scalar_correctness() {
        let a = [0xFFFF_FFFF_FFFF_FFFFu64, 0x0];
        let b = [0xFFFF_FFFF_FFFF_FFFFu64, 0x0];
        let c = [0xFFFF_FFFF_FFFF_FFFFu64, 0xFFFF_FFFF_FFFF_FFFFu64];
        assert_eq!(hamming_scalar(&a, &b), 0);
        assert_eq!(hamming_scalar(&a, &c), 64);
    }

    #[test]
    fn smoke_1024_ok() {
        assert!(smoke_1024());
    }

    // (caps-injection test removed: SELECTED is a global static and parallel
    // tests race; smoke_1024 covers kernel correctness)

    #[cfg(target_arch = "x86_64")]
    fn deterministic_words(seed: &mut u64, n: usize) -> Vec<u64> {
        // LCG xorshift64* — pseudo-aleatório determinístico (paridade host)
        (0..n)
            .map(|_| {
                *seed ^= *seed << 13;
                *seed ^= *seed >> 7;
                *seed ^= *seed << 17;
                *seed
            })
            .collect()
    }

    #[test]
    fn differential_scalar_vs_all_kernels() {
        // P0-10: todo kernel SIMD (se suportado) deve concordar EXATAMENTE com
        // hamming_scalar — qualquer divergência = bug de dispatch/kernel.
        // Cobertura de comprimentos: 1..17 words (cobre tails 4/8 + tail loop
        // scalar + longer[n..]) e pares com lengths DESIGUAIS (a!=b).
        let mut seed = 0xDEAD_BEEF_CAFE_F00Du64;
        for n in 1..=17u64 {
            let (a, b) = (
                deterministic_words(&mut seed, n as usize),
                deterministic_words(&mut seed, n as usize),
            );
            let expect = hamming_scalar(&a, &b);
            if cpu_has_avx2() {
                // SAFETY: cpu_has_avx2() verificado acima (requisito do kernel)
                let got = unsafe { hamming_avx2_xor(&a, &b) };
                assert_eq!(got, expect, "avx2 divergiu no len {n}");
            }
            if cpu_has_avx512() {
                // SAFETY: cpu_has_avx512() verificado acima (requisito do kernel)
                let got = unsafe { hamming_avx512_dispatch(&a, &b) };
                assert_eq!(got, expect, "avx512 divergiu no len {n}");
            }
        }
        // lengths desiguais: só o segmento comum é XOR; o resto do maior entra
        let mut a = deterministic_words(&mut seed, 13);
        let mut b = deterministic_words(&mut seed, 5);
        let expect = hamming_scalar(&a, &b);
        if cpu_has_avx2() {
            // SAFETY: cpu_has_avx2() verificado acima
            let got = unsafe { hamming_avx2_xor(&a, &b) };
            assert_eq!(got, expect, "avx2 divergiu (len desiguais)");
        }
        if cpu_has_avx512() {
            // SAFETY: cpu_has_avx512() verificado acima
            let got = unsafe { hamming_avx512_dispatch(&a, &b) };
            assert_eq!(got, expect, "avx512 divergiu (len desiguais)");
        }
        a.clear();
        b.clear();
        let expect = hamming_scalar(&a, &b);
        assert_eq!(expect, 0, "scalar vazio deve ser 0");
    }
}
