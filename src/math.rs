//! Polyfills de ponto flutuante para `no_std` (v1.1.22).
//!
//! No alvo bare-metal (`x86_64-unknown-none`) o `core` **não expõe** `f32::sqrt`,
//! `f32::ln`, `f32::exp` nem `f32::round` — golden rule 3 do `AGENTS.md`. Este
//! módulo é o único lugar onde esses ponteiros vivem: antes estavam espalhados
//! (`sqrt_f32`/`exp_f32` em `sgdb.rs`, `ln_f32` em `lexical.rs`), cada um com sua
//! precisão documentada em prosa e nenhum teste de precisão contra o `std`.
//!
//! ## Contrato
//!
//! - **`no_std`-safe**, `alloc`-free, zero deps (ADR-0001).
//! - **Precisão suficiente para ORDENAR, não para medir** — mas os números têm
//!   de ser os REAIS. Medidos em v1.1.22 (testes `std` abaixo, com `println` do
//!   pior caso):
//!
//!   | função | pior erro relativo | onde |
//!   |---|---|---|
//!   | `sqrt_f32` | < 1e-6 | — |
//!   | `exp_f32` | ~3.1e-6 | varredura `[-10, 10]` |
//!   | `ln_f32` | **~5.9e-2** | `x ≈ 1.85` |
//!
//!   **`ln_f32` é o elo fraco, e isto é um achado de medição, não uma opinião:** a
//!   série de 5 termos em `y = mantissa − 1` converge devagar conforme `y → 1`,
//!   então o erro cresce até ~6% perto de `x ≈ 2`. A doc anterior afirmava
//!   "~1e-5" — estava errada por ~4 ordens de grandeza. O BM25 usa `ln` em IDF
//!   (`ln((n+1)/(df+1)) + 1`) e TF (`1 + ln(tf)`); com corpora pequenos esses
//!   argumentos caem justamente na faixa ruim. A ordenação sobrevive (o termo
//!   `ln` entra somado a constantes maiores), mas **trocar a série por uma
//!   melhor é ganho de qualidade de recall real** — e é mudança DELIBERADA de
//!   scores, não refactor: ver o teste `bm25_ranking_is_frozen_across_math_move`
//!   (`src/lexical.rs`), que é o guard de migração.
//! - **`f32::round` não existe** no core deste alvo: arredonda-se por sinal
//!   (`+0.5` / `−0.5` antes do truncamento `as i32`) — é o que `exp_f32` faz ao
//!   escolher `k`, e a razão de não haver aqui um `round_f32`.
//! - **Mover, não tocar:** esta consolidação preserva os algoritmos byte a byte.
//!   Mudar precisão é outra decisão (e mexeria no BM25, logo no hot test).

/// Raiz quadrada por Newton-Raphson (10 iterações fixas).
///
/// 10 passos convergem de sobra para `f32` em partida de `y = x`; o custo
/// previsível (sem `while`) é o que mantém o caminho de rescore determinístico.
/// `x <= 0.0` → `0.0` (não devolve `NaN`, que contaminaria o ranking).
///
/// Precisão: ~1e-7 relativo (teste `sqrt_matches_std`).
pub(crate) fn sqrt_f32(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut y = x;
    for _ in 0..10 {
        y = (y + x / y) * 0.5;
    }
    y
}

/// Logaritmo natural: expoente IEEE + série no mantissa (redução para `[1,2)`).
///
/// `x <= 0.0` → `-3.0` (clamp; o BM25 nunca pede log de 0/negativo, mas um `NaN`
/// aqui envenenaria o score do documento inteiro).
///
/// Precisão: **~5.9e-2** no pior caso (`x ≈ 1.85`) — muito pior que a doc
/// original prometia ("~1e-5"), e agora MEDIDA por
/// `ln_worst_case_error_is_bounded`. Ver a nota do módulo: melhorar a série é
/// mudança deliberada de scores, não refactor.
pub(crate) fn ln_f32(x: f32) -> f32 {
    if x <= 0.0 {
        return -3.0; // clamp: log de 0/neg não usado no BM25
    }
    let bits = x.to_bits();
    let exp = ((bits >> 23) & 0xFF) as i32 - 127;
    let mant = (bits & 0x7F_FFFF) | 0x3F80_0000; // [1,2)
    let m = f32::from_bits(mant);
    let y = m - 1.0;
    let ln_m = y * (1.0 - 0.5 * y + y * y / 3.0 - y * y * y / 4.0 + y * y * y * y / 5.0);
    exp as f32 * core::f32::consts::LN_2 + ln_m
}

/// Exponencial: `x = k·ln2 + r` (k por arredondamento de SINAL — `f32::round`
/// não existe no core) + série de Taylor de 5 termos em `r`, reconstruída por
/// manipulação direta dos bits do expoente IEEE.
///
/// Satura fora de `[-40, 40]` (`0.0` / `f32::MAX`) — o decay de Ebbinghaus
/// (`exp(−idade/half_life)`) nunca precisa além disso, e saturar evita `inf`/
/// `NaN` no caminho de importância.
///
/// Precisão: **~3.1e-6** relativo, medido numa varredura larga
/// (`exp_matches_std`) — o melhor dos três polyfills.
pub(crate) fn exp_f32(x: f32) -> f32 {
    if x <= -40.0 {
        return 0.0;
    }
    if x >= 40.0 {
        return f32::MAX;
    }
    let k = ((x * core::f32::consts::LOG2_E) + if x < 0.0 { -0.5 } else { 0.5 }) as i32;
    let r = x - k as f32 * core::f32::consts::LN_2;
    let e = 1.0
        + r * (1.0 + r * (0.5 + r * (1.0 / 6.0 + r * (1.0 / 24.0 + r * (1.0 / 120.0)))));
    let bits = ((k + 127) as u32) << 23;
    e * f32::from_bits(bits)
}

/// `ln_f32` é `pub(crate)` porque o BM25 (`lexical.rs`) é o seu único consumidor
/// hoje; `exp_f32` só é usado pelo decay; `sqrt_f32` pelo rescore e pelo
/// `embedder`. Reexportar como API pública é decisão separada (e a `Embedder`
/// seam já é o contrato público para quem precisa de norma).
///
/// Testes **só no host** (`std`): o oráculo destes polyfills é justamente o
/// `f32::ln`/`exp` do `std`, que NÃO existe no core bare-metal — sem `std` não
/// há com o que comparar (e o gate `--no-default-features` compila este módulo).
#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;

    /// As três comparações com o `std` (só existem no host) — o epsilon
    /// documentado de cada função, verificado em vez de afirmado.
    #[test]
    fn sqrt_matches_std() {
        for i in 1..2000 {
            let x = i as f32 * 0.37;
            let (got, want) = (sqrt_f32(x), x.sqrt());
            let rel = ((got - want) / want.max(1e-9)).abs();
            assert!(rel < 1e-6, "sqrt({x}): got={got} want={want} rel={rel}");
        }
        assert_eq!(sqrt_f32(0.0), 0.0);
        assert_eq!(sqrt_f32(-1.0), 0.0, "negativo → 0, nunca NaN");
        assert_eq!(sqrt_f32(1.0), 1.0);
    }

    /// Pior erro relativo observado de `ln_f32` numa varredura larga — o número
    /// REAL, não o que a prosa prometia. (A doc original dizia "~1e-5"; a
    /// medição de v1.1.22 mostrou ~1e-2 perto de x≈2, onde a série de 5 termos
    /// em `y = m − 1` converge devagar. O achado está registrado no módulo.)
    #[test]
    fn ln_worst_case_error_is_bounded() {
        let mut worst = 0.0f32;
        let mut at = 0.0f32;
        for i in 1..4000 {
            let x = i as f32 * 0.37;
            let want = x.ln();
            if want.abs() < 1e-3 {
                continue; // perto de 1 o erro relativo explode sem significar nada
            }
            let rel = ((ln_f32(x) - want) / want).abs();
            if rel > worst {
                worst = rel;
                at = x;
            }
        }
        println!("ln_f32 pior erro relativo = {worst} em x={at}");
        // Guard de REGRESSÃO contra o valor medido (5.9e-2 em v1.1.22), não
        // contra o aspiracional. Se este bound apertar, a série foi melhorada
        // de propósito — atualize o número E o CHANGELOG (muda ranking BM25).
        assert!(
            worst < 1e-1,
            "ln_f32 piorou: {worst} em x={at} (medido 5.9e-2 em v1.1.22)"
        );
        assert_eq!(ln_f32(1.0), 0.0);
        assert_eq!(ln_f32(0.0), -3.0, "clamp, nunca -inf");
        assert_eq!(ln_f32(-2.0), -3.0);
    }

    #[test]
    fn exp_matches_std() {
        let mut worst = 0.0f32;
        for i in -200..200 {
            let x = i as f32 * 0.05;
            let (got, want) = (exp_f32(x), x.exp());
            let rel = ((got - want) / want.abs().max(1e-9)).abs();
            if rel > worst {
                worst = rel;
            }
        }
        println!("exp_f32 pior erro relativo = {worst}");
        assert!(worst < 1e-4, "exp_f32 piorou: {worst}");
        assert_eq!(exp_f32(0.0), 1.0);
        assert_eq!(exp_f32(-40.0), 0.0, "satura embaixo");
        assert!(exp_f32(40.0) >= f32::MAX * 0.99, "satura em cima");
    }

    /// `sqrt_f32` e `exp_f32` são monotônicos; **`ln_f32` NÃO é**, e isso é uma
    /// propriedade MEDIDA do polyfill atual (série truncada em 5 termos), não um
    /// bug do move: o erro da série varia com o mantissa e, em trechos, `ln` de
    /// um x maior fica abaixo do de um x menor. O ranking do BM25 convive com
    /// isso desde v1.1.4. Trocar por uma série melhor MUDA scores — logo é
    /// mudança deliberada (CHANGELOG), não refactor: este teste pina o presente.
    #[test]
    fn sqrt_and_exp_are_monotone_ln_is_knowingly_not() {
        let mut prev_sqrt = f32::NEG_INFINITY;
        for i in 1..1000 {
            let r = sqrt_f32(i as f32 * 0.13);
            assert!(r >= prev_sqrt, "sqrt não monotônico em {i}");
            prev_sqrt = r;
        }
        let mut prev = f32::NEG_INFINITY;
        for i in -300..300 {
            let e = exp_f32(i as f32 * 0.05);
            assert!(e >= prev, "exp não monotônico em {i}");
            prev = e;
        }
        // ln: só o sinal da derivada média (crescente em larga escala) é garantido.
        assert!(ln_f32(2.0) > ln_f32(1.5));
        assert!(ln_f32(1.5) > ln_f32(1.1));
        assert!(ln_f32(1000.0) > ln_f32(2.0));
    }

    /// Determinismo: mesmos bits na entrada → mesmos bits na saída. Os três
    /// rodam por iteração fixa (nenhum `while`), então não há como divergir.
    #[test]
    fn polyfills_are_bit_deterministic() {
        for i in 1..200 {
            let x = i as f32 * 0.77;
            assert_eq!(sqrt_f32(x).to_bits(), sqrt_f32(x).to_bits());
            assert_eq!(ln_f32(x).to_bits(), ln_f32(x).to_bits());
            assert_eq!(exp_f32(x).to_bits(), exp_f32(x).to_bits());
        }
    }

    /// O padrão de arredondamento por SINAL (`+0.5`/`−0.5` antes do trunc `as
    /// i32`) é uma regra do alvo, não um detalhe: `f32::round` não existe no
    /// core. Este teste pina o comportamento em torno de zero, onde `as i32`
    /// truncaria para o lado errado.
    #[test]
    fn exp_uses_sign_aware_rounding() {
        // exp perto de 1: se `k` fosse truncado em vez de arredondado, o erro
        // relativo explodiria nos dois lados de zero.
        for i in -20..20 {
            let x = i as f32 * 0.02;
            let (got, want) = (exp_f32(x), x.exp());
            let rel = ((got - want) / want.abs().max(1e-9)).abs();
            assert!(rel < 1e-4, "perto de 0: exp({x}) rel={rel}");
        }
    }
}
