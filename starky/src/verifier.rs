use alloc::vec::Vec;
use core::iter::once;

use anyhow::{anyhow, ensure, Result};
use itertools::Itertools;
use plonky2::field::extension::{Extendable, FieldExtension};
use plonky2::field::types::Field;
use plonky2::fri::verifier::verify_fri_proof;
use plonky2::hash::hash_types::RichField;
use plonky2::hash::merkle_tree::MerkleCap;
use plonky2::plonk::config::GenericConfig;
use plonky2::plonk::plonk_common::reduce_with_powers;

use crate::config::StarkConfig;
use crate::constraint_consumer::ConstraintConsumer;
use crate::evaluation_frame::StarkEvaluationFrame;
use crate::lookup::LookupCheckVars;
use crate::proof::{StarkOpeningSet, StarkProof, StarkProofChallenges, StarkProofWithPublicInputs};
use crate::stark::Stark;
use crate::vanishing_poly::eval_vanishing_poly;

pub fn verify_stark_proof<
    F: RichField + Extendable<D>,
    C: GenericConfig<D, F = F>,
    S: Stark<F, D>,
    const D: usize,
>(
    stark: S,
    proof_with_pis: StarkProofWithPublicInputs<F, C, D>,
    config: &StarkConfig,
) -> Result<()> {
    ensure!(proof_with_pis.public_inputs.len() == S::PUBLIC_INPUTS);
    let degree_bits = proof_with_pis.proof.recover_degree_bits(config);
    let challenges = proof_with_pis.get_challenges(config, degree_bits);
    verify_stark_proof_with_challenges(stark, proof_with_pis, challenges, degree_bits, config)
}

pub(crate) fn verify_stark_proof_with_challenges<
    F: RichField + Extendable<D>,
    C: GenericConfig<D, F = F>,
    S: Stark<F, D>,
    const D: usize,
>(
    stark: S,
    proof_with_pis: StarkProofWithPublicInputs<F, C, D>,
    challenges: StarkProofChallenges<F, D>,
    degree_bits: usize,
    config: &StarkConfig,
) -> Result<()> {
    validate_proof_shape(&stark, &proof_with_pis, config)?;

    let StarkProofWithPublicInputs {
        proof,
        public_inputs,
    } = proof_with_pis;
    let StarkOpeningSet {
        local_values,
        next_values,
        auxiliary_polys,
        auxiliary_polys_next,
        quotient_polys,
    } = &proof.openings;
    let vars = S::EvaluationFrame::from_values(
        local_values,
        next_values,
        &public_inputs
            .iter()
            .copied()
            .map(F::Extension::from_basefield)
            .collect::<Vec<_>>(),
    );
    let (l_0, l_last) = eval_l_0_and_l_last(degree_bits, challenges.stark_zeta);
    let last = F::primitive_root_of_unity(degree_bits).inverse();
    let z_last = challenges.stark_zeta - last.into();
    let mut consumer = ConstraintConsumer::<F::Extension>::new(
        challenges
            .stark_alphas
            .iter()
            .map(|&alpha| F::Extension::from_basefield(alpha))
            .collect::<Vec<_>>(),
        z_last,
        l_0,
        l_last,
    );

    let num_lookup_columns = stark.num_lookup_helper_columns(config);
    let lookup_challenges = (num_lookup_columns > 0).then(|| {
        challenges
            .lookup_challenge_set
            .unwrap()
            .challenges
            .iter()
            .map(|ch| ch.beta)
            .collect::<Vec<_>>()
    });

    let lookup_vars = stark.uses_lookups().then(|| LookupCheckVars {
        local_values: auxiliary_polys.as_ref().unwrap().clone(),
        next_values: auxiliary_polys_next.as_ref().unwrap().clone(),
        challenges: lookup_challenges.unwrap(),
    });
    let lookups = stark.lookups();

    eval_vanishing_poly::<F, F::Extension, F::Extension, S, D, D>(
        &stark,
        &vars,
        &lookups,
        lookup_vars,
        &mut consumer,
    );
    let vanishing_polys_zeta = consumer.accumulators();

    // Check each polynomial identity, of the form `vanishing(x) = Z_H(x) quotient(x)`, at zeta.
    let zeta_pow_deg = challenges.stark_zeta.exp_power_of_2(degree_bits);
    let z_h_zeta = zeta_pow_deg - F::Extension::ONE;
    // `quotient_polys_zeta` holds `num_challenges * quotient_degree_factor` evaluations.
    // Each chunk of `quotient_degree_factor` holds the evaluations of `t_0(zeta),...,t_{quotient_degree_factor-1}(zeta)`
    // where the "real" quotient polynomial is `t(X) = t_0(X) + t_1(X)*X^n + t_2(X)*X^{2n} + ...`.
    // So to reconstruct `t(zeta)` we can compute `reduce_with_powers(chunk, zeta^n)` for each
    // `quotient_degree_factor`-sized chunk of the original evaluations.
    for (i, chunk) in quotient_polys
        .chunks(stark.quotient_degree_factor())
        .enumerate()
    {
        ensure!(
            vanishing_polys_zeta[i] == z_h_zeta * reduce_with_powers(chunk, zeta_pow_deg),
            "Mismatch between evaluation and opening of quotient polynomial"
        );
    }

    let merkle_caps = once(proof.trace_cap)
        .chain(proof.auxiliary_polys_cap)
        .chain(once(proof.quotient_polys_cap))
        .collect_vec();

    verify_fri_proof::<F, C, D>(
        &stark.fri_instance(
            challenges.stark_zeta,
            F::primitive_root_of_unity(degree_bits),
            config,
        ),
        &proof.openings.to_fri_openings(),
        &challenges.fri_challenges,
        &merkle_caps,
        &proof.opening_proof,
        &config.fri_params(degree_bits),
    )?;

    Ok(())
}

fn validate_proof_shape<F, C, S, const D: usize>(
    stark: &S,
    proof_with_pis: &StarkProofWithPublicInputs<F, C, D>,
    config: &StarkConfig,
) -> anyhow::Result<()>
where
    F: RichField + Extendable<D>,
    C: GenericConfig<D, F = F>,
    S: Stark<F, D>,
{
    let StarkProofWithPublicInputs {
        proof,
        public_inputs,
    } = proof_with_pis;
    let degree_bits = proof.recover_degree_bits(config);

    let StarkProof {
        trace_cap,
        auxiliary_polys_cap,
        quotient_polys_cap,
        openings,
        // The shape of the opening proof will be checked in the FRI verifier (see
        // validate_fri_proof_shape), so we ignore it here.
        opening_proof: _,
    } = proof;

    let StarkOpeningSet {
        local_values,
        next_values,
        auxiliary_polys,
        auxiliary_polys_next,
        quotient_polys,
    } = openings;

    ensure!(public_inputs.len() == S::PUBLIC_INPUTS);

    let fri_params = config.fri_params(degree_bits);
    let cap_height = fri_params.config.cap_height;

    let num_auxiliary = stark.num_lookup_helper_columns(config);

    ensure!(trace_cap.height() == cap_height);
    ensure!(quotient_polys_cap.height() == cap_height);

    ensure!(local_values.len() == S::COLUMNS);
    ensure!(next_values.len() == S::COLUMNS);
    ensure!(quotient_polys.len() == stark.num_quotient_polys(config));

    check_lookup_options::<F, C, S, D>(
        stark,
        auxiliary_polys_cap,
        auxiliary_polys,
        auxiliary_polys_next,
        config,
    )?;

    Ok(())
}

/// Evaluate the Lagrange polynomials `L_0` and `L_(n-1)` at a point `x`.
/// `L_0(x) = (x^n - 1)/(n * (x - 1))`
/// `L_(n-1)(x) = (x^n - 1)/(n * (g * x - 1))`, with `g` the first element of the subgroup.
fn eval_l_0_and_l_last<F: Field>(log_n: usize, x: F) -> (F, F) {
    let n = F::from_canonical_usize(1 << log_n);
    let g = F::primitive_root_of_unity(log_n);
    let z_x = x.exp_power_of_2(log_n) - F::ONE;
    let invs = F::batch_multiplicative_inverse(&[n * (x - F::ONE), n * (g * x - F::ONE)]);

    (z_x * invs[0], z_x * invs[1])
}

/// Utility function to check that all lookups data wrapped in `Option`s are `Some` iff
/// the Stark uses a permutation argument.
fn check_lookup_options<
    F: RichField + Extendable<D>,
    C: GenericConfig<D, F = F>,
    S: Stark<F, D>,
    const D: usize,
>(
    stark: &S,
    auxiliary_polys_cap: &Option<MerkleCap<F, <C as GenericConfig<D>>::Hasher>>,
    auxiliary_polys: &Option<Vec<<F as Extendable<D>>::Extension>>,
    auxiliary_polys_next: &Option<Vec<<F as Extendable<D>>::Extension>>,
    config: &StarkConfig,
) -> Result<()> {
    if stark.uses_lookups() {
        let num_auxiliary = stark.num_lookup_helper_columns(config);
        let cap_height = config.fri_config.cap_height;

        let auxiliary_polys_cap = auxiliary_polys_cap
            .as_ref()
            .ok_or_else(|| anyhow!("Missing auxiliary_polys_cap"))?;
        let auxiliary_polys = auxiliary_polys
            .as_ref()
            .ok_or_else(|| anyhow!("Missing auxiliary_polys"))?;
        let auxiliary_polys_next = auxiliary_polys_next
            .as_ref()
            .ok_or_else(|| anyhow!("Missing auxiliary_polys_next"))?;

        ensure!(auxiliary_polys_cap.height() == cap_height);
        ensure!(auxiliary_polys.len() == num_auxiliary);
        ensure!(auxiliary_polys_next.len() == num_auxiliary);
    } else {
        ensure!(auxiliary_polys_cap.is_none());
        ensure!(auxiliary_polys.is_none());
        ensure!(auxiliary_polys_next.is_none());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use plonky2::field::goldilocks_field::GoldilocksField;
    use plonky2::field::polynomial::PolynomialValues;
    use plonky2::field::types::Sample;

    use crate::verifier::eval_l_0_and_l_last;

    #[test]
    fn test_eval_l_0_and_l_last() {
        type F = GoldilocksField;
        let log_n = 5;
        let n = 1 << log_n;

        let x = F::rand(); // challenge point
        let expected_l_first_x = PolynomialValues::selector(n, 0).ifft().eval(x);
        let expected_l_last_x = PolynomialValues::selector(n, n - 1).ifft().eval(x);

        let (l_first_x, l_last_x) = eval_l_0_and_l_last(log_n, x);
        assert_eq!(l_first_x, expected_l_first_x);
        assert_eq!(l_last_x, expected_l_last_x);
    }
}
