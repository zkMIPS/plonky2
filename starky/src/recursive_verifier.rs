use alloc::vec;
use alloc::vec::Vec;
use core::iter::once;

use anyhow::{ensure, Result};
use itertools::Itertools;
use plonky2::field::extension::Extendable;
use plonky2::field::types::Field;
use plonky2::fri::witness_util::set_fri_proof_target;
use plonky2::hash::hash_types::RichField;
use plonky2::iop::ext_target::ExtensionTarget;
use plonky2::iop::witness::Witness;
use plonky2::plonk::circuit_builder::CircuitBuilder;
use plonky2::plonk::config::{AlgebraicHasher, GenericConfig};
use plonky2::util::reducing::ReducingFactorTarget;
use plonky2::with_context;

use crate::config::StarkConfig;
use crate::constraint_consumer::RecursiveConstraintConsumer;
use crate::evaluation_frame::StarkEvaluationFrame;
use crate::lookup::LookupCheckVarsTarget;
use crate::proof::{
    StarkOpeningSetTarget, StarkProof, StarkProofChallengesTarget, StarkProofTarget,
    StarkProofWithPublicInputs, StarkProofWithPublicInputsTarget,
};
use crate::stark::Stark;
use crate::vanishing_poly::eval_vanishing_poly_circuit;

pub fn verify_stark_proof_circuit<
    F: RichField + Extendable<D>,
    C: GenericConfig<D, F = F>,
    S: Stark<F, D>,
    const D: usize,
>(
    builder: &mut CircuitBuilder<F, D>,
    stark: S,
    proof_with_pis: StarkProofWithPublicInputsTarget<D>,
    inner_config: &StarkConfig,
) where
    C::Hasher: AlgebraicHasher<F>,
{
    assert_eq!(proof_with_pis.public_inputs.len(), S::PUBLIC_INPUTS);
    let degree_bits = proof_with_pis.proof.recover_degree_bits(inner_config);
    let challenges = with_context!(
        builder,
        "compute challenges",
        proof_with_pis.get_challenges::<F, C>(builder, inner_config)
    );

    verify_stark_proof_with_challenges_circuit::<F, C, S, D>(
        builder,
        stark,
        proof_with_pis,
        challenges,
        inner_config,
        degree_bits,
    );
}

/// Recursively verifies an inner proof.
fn verify_stark_proof_with_challenges_circuit<
    F: RichField + Extendable<D>,
    C: GenericConfig<D, F = F>,
    S: Stark<F, D>,
    const D: usize,
>(
    builder: &mut CircuitBuilder<F, D>,
    stark: S,
    proof_with_pis: StarkProofWithPublicInputsTarget<D>,
    challenges: StarkProofChallengesTarget<D>,
    inner_config: &StarkConfig,
    degree_bits: usize,
) where
    C::Hasher: AlgebraicHasher<F>,
{
    check_lookup_options(&stark, &proof_with_pis, &challenges).unwrap();
    let one = builder.one_extension();

    let StarkProofWithPublicInputsTarget {
        proof,
        public_inputs,
    } = proof_with_pis;
    let StarkOpeningSetTarget {
        local_values,
        next_values,
        auxiliary_polys,
        auxiliary_polys_next,
        quotient_polys,
    } = &proof.openings;

    let vars = S::EvaluationFrameTarget::from_values(
        local_values,
        next_values,
        &public_inputs
            .into_iter()
            .map(|t| builder.convert_to_ext(t))
            .collect::<Vec<_>>(),
    );

    let zeta_pow_deg = builder.exp_power_of_2_extension(challenges.stark_zeta, degree_bits);
    let z_h_zeta = builder.sub_extension(zeta_pow_deg, one);
    let (l_0, l_last) =
        eval_l_0_and_l_last_circuit(builder, degree_bits, challenges.stark_zeta, z_h_zeta);
    let last =
        builder.constant_extension(F::Extension::primitive_root_of_unity(degree_bits).inverse());
    let z_last = builder.sub_extension(challenges.stark_zeta, last);

    let mut consumer = RecursiveConstraintConsumer::<F, D>::new(
        builder.zero_extension(),
        challenges.stark_alphas,
        z_last,
        l_0,
        l_last,
    );

    let num_lookup_columns = stark.num_lookup_helper_columns(inner_config);
    let lookup_challenges = stark.uses_lookups().then(|| {
        challenges
            .lookup_challenge_set
            .unwrap()
            .challenges
            .iter()
            .map(|ch| ch.beta)
            .collect::<Vec<_>>()
    });

    let lookup_vars = stark.uses_lookups().then(|| LookupCheckVarsTarget {
        local_values: auxiliary_polys.as_ref().unwrap()[..num_lookup_columns].to_vec(),
        next_values: auxiliary_polys_next.as_ref().unwrap()[..num_lookup_columns].to_vec(),
        challenges: lookup_challenges.unwrap(),
    });

    with_context!(
        builder,
        "evaluate vanishing polynomial",
        eval_vanishing_poly_circuit::<F, S, D>(builder, &stark, &vars, lookup_vars, &mut consumer)
    );
    let vanishing_polys_zeta = consumer.accumulators();

    // Check each polynomial identity, of the form `vanishing(x) = Z_H(x) quotient(x)`, at zeta.
    let mut scale = ReducingFactorTarget::new(zeta_pow_deg);
    for (i, chunk) in quotient_polys
        .chunks(stark.quotient_degree_factor())
        .enumerate()
    {
        let recombined_quotient = scale.reduce(chunk, builder);
        let computed_vanishing_poly = builder.mul_extension(z_h_zeta, recombined_quotient);
        builder.connect_extension(vanishing_polys_zeta[i], computed_vanishing_poly);
    }

    let merkle_caps = once(proof.trace_cap)
        .chain(proof.auxiliary_polys_cap)
        .chain(once(proof.quotient_polys_cap))
        .collect_vec();

    let fri_instance = stark.fri_instance_target(
        builder,
        challenges.stark_zeta,
        F::primitive_root_of_unity(degree_bits),
        inner_config,
    );
    builder.verify_fri_proof::<C>(
        &fri_instance,
        &proof.openings.to_fri_openings(),
        &challenges.fri_challenges,
        &merkle_caps,
        &proof.opening_proof,
        &inner_config.fri_params(degree_bits),
    );
}

fn eval_l_0_and_l_last_circuit<F: RichField + Extendable<D>, const D: usize>(
    builder: &mut CircuitBuilder<F, D>,
    log_n: usize,
    x: ExtensionTarget<D>,
    z_x: ExtensionTarget<D>,
) -> (ExtensionTarget<D>, ExtensionTarget<D>) {
    let n = builder.constant_extension(F::Extension::from_canonical_usize(1 << log_n));
    let g = builder.constant_extension(F::Extension::primitive_root_of_unity(log_n));
    let one = builder.one_extension();
    let l_0_deno = builder.mul_sub_extension(n, x, n);
    let l_last_deno = builder.mul_sub_extension(g, x, one);
    let l_last_deno = builder.mul_extension(n, l_last_deno);

    (
        builder.div_extension(z_x, l_0_deno),
        builder.div_extension(z_x, l_last_deno),
    )
}

pub fn add_virtual_stark_proof_with_pis<
    F: RichField + Extendable<D>,
    S: Stark<F, D>,
    const D: usize,
>(
    builder: &mut CircuitBuilder<F, D>,
    stark: S,
    config: &StarkConfig,
    degree_bits: usize,
) -> StarkProofWithPublicInputsTarget<D> {
    let proof = add_virtual_stark_proof::<F, S, D>(builder, stark, config, degree_bits);
    let public_inputs = builder.add_virtual_targets(S::PUBLIC_INPUTS);
    StarkProofWithPublicInputsTarget {
        proof,
        public_inputs,
    }
}

pub fn add_virtual_stark_proof<F: RichField + Extendable<D>, S: Stark<F, D>, const D: usize>(
    builder: &mut CircuitBuilder<F, D>,
    stark: S,
    config: &StarkConfig,
    degree_bits: usize,
) -> StarkProofTarget<D> {
    let fri_params = config.fri_params(degree_bits);
    let cap_height = fri_params.config.cap_height;

    let num_leaves_per_oracle = vec![
        S::COLUMNS,
        stark.num_lookup_helper_columns(config),
        stark.quotient_degree_factor() * config.num_challenges,
    ];

    let auxiliary_polys_cap = stark
        .uses_lookups()
        .then(|| builder.add_virtual_cap(cap_height));

    StarkProofTarget {
        trace_cap: builder.add_virtual_cap(cap_height),
        auxiliary_polys_cap,
        quotient_polys_cap: builder.add_virtual_cap(cap_height),
        openings: add_stark_opening_set_target::<F, S, D>(builder, stark, config),
        opening_proof: builder.add_virtual_fri_proof(&num_leaves_per_oracle, &fri_params),
    }
}

fn add_stark_opening_set_target<F: RichField + Extendable<D>, S: Stark<F, D>, const D: usize>(
    builder: &mut CircuitBuilder<F, D>,
    stark: S,
    config: &StarkConfig,
) -> StarkOpeningSetTarget<D> {
    let num_challenges = config.num_challenges;
    StarkOpeningSetTarget {
        local_values: builder.add_virtual_extension_targets(S::COLUMNS),
        next_values: builder.add_virtual_extension_targets(S::COLUMNS),
        auxiliary_polys: stark.uses_lookups().then(|| {
            builder.add_virtual_extension_targets(stark.num_lookup_helper_columns(config))
        }),
        auxiliary_polys_next: stark.uses_lookups().then(|| {
            builder.add_virtual_extension_targets(stark.num_lookup_helper_columns(config))
        }),
        quotient_polys: builder
            .add_virtual_extension_targets(stark.quotient_degree_factor() * num_challenges),
    }
}

pub fn set_stark_proof_with_pis_target<F, C: GenericConfig<D, F = F>, W, const D: usize>(
    witness: &mut W,
    stark_proof_with_pis_target: &StarkProofWithPublicInputsTarget<D>,
    stark_proof_with_pis: &StarkProofWithPublicInputs<F, C, D>,
) where
    F: RichField + Extendable<D>,
    C::Hasher: AlgebraicHasher<F>,
    W: Witness<F>,
{
    let StarkProofWithPublicInputs {
        proof,
        public_inputs,
    } = stark_proof_with_pis;
    let StarkProofWithPublicInputsTarget {
        proof: pt,
        public_inputs: pi_targets,
    } = stark_proof_with_pis_target;

    // Set public inputs.
    for (&pi_t, &pi) in pi_targets.iter().zip_eq(public_inputs) {
        witness.set_target(pi_t, pi);
    }

    set_stark_proof_target(witness, pt, proof);
}

pub fn set_stark_proof_target<F, C: GenericConfig<D, F = F>, W, const D: usize>(
    witness: &mut W,
    proof_target: &StarkProofTarget<D>,
    proof: &StarkProof<F, C, D>,
) where
    F: RichField + Extendable<D>,
    C::Hasher: AlgebraicHasher<F>,
    W: Witness<F>,
{
    witness.set_cap_target(&proof_target.trace_cap, &proof.trace_cap);
    witness.set_cap_target(&proof_target.quotient_polys_cap, &proof.quotient_polys_cap);

    witness.set_fri_openings(
        &proof_target.openings.to_fri_openings(),
        &proof.openings.to_fri_openings(),
    );

    if let (Some(auxiliary_polys_cap_target), Some(auxiliary_polys_cap)) = (
        &proof_target.auxiliary_polys_cap,
        &proof.auxiliary_polys_cap,
    ) {
        witness.set_cap_target(auxiliary_polys_cap_target, auxiliary_polys_cap);
    }

    set_fri_proof_target(witness, &proof_target.opening_proof, &proof.opening_proof);
}

/// Utility function to check that all lookups data wrapped in `Option`s are `Some` iff
/// the Stark uses a permutation argument.
fn check_lookup_options<F: RichField + Extendable<D>, S: Stark<F, D>, const D: usize>(
    stark: &S,
    proof_with_pis: &StarkProofWithPublicInputsTarget<D>,
    challenges: &StarkProofChallengesTarget<D>,
) -> Result<()> {
    let options_is_some = [
        proof_with_pis.proof.auxiliary_polys_cap.is_some(),
        proof_with_pis.proof.openings.auxiliary_polys.is_some(),
        proof_with_pis.proof.openings.auxiliary_polys_next.is_some(),
        challenges.lookup_challenge_set.is_some(),
    ];
    ensure!(
        options_is_some
            .into_iter()
            .all(|b| b == stark.uses_lookups()),
        "Lookups data doesn't match with Stark configuration."
    );
    Ok(())
}
