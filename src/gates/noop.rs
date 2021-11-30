use crate::field::extension_field::target::ExtensionTarget;
use crate::field::extension_field::Extendable;
use crate::field::field_types::RichField;
use crate::gates::gate::Gate;
use crate::iop::generator::WitnessGenerator;
use crate::plonk::circuit_builder::CircuitBuilder;
use crate::plonk::vars::{EvaluationTargets, EvaluationVars, EvaluationVarsBaseBatch};

/// A gate which does nothing.
pub struct NoopGate;

impl<F: RichField + Extendable<D>, const D: usize> Gate<F, D> for NoopGate {
    fn id(&self) -> String {
        "NoopGate".into()
    }

    fn eval_unfiltered(&self, _vars: EvaluationVars<F, D>) -> Vec<F::Extension> {
        Vec::new()
    }

    fn eval_unfiltered_base_batch(&self, vars_base: EvaluationVarsBaseBatch<F>) -> Vec<F> {
        Vec::new()
    }

    fn eval_unfiltered_recursively(
        &self,
        _builder: &mut CircuitBuilder<F, D>,
        _vars: EvaluationTargets<D>,
    ) -> Vec<ExtensionTarget<D>> {
        Vec::new()
    }

    fn generators(
        &self,
        _gate_index: usize,
        _local_constants: &[F],
    ) -> Vec<Box<dyn WitnessGenerator<F>>> {
        Vec::new()
    }

    fn num_wires(&self) -> usize {
        0
    }

    fn num_constants(&self) -> usize {
        0
    }

    fn degree(&self) -> usize {
        0
    }

    fn num_constraints(&self) -> usize {
        0
    }
}

#[cfg(test)]
mod tests {
    use crate::field::goldilocks_field::GoldilocksField;
    use crate::gates::gate_testing::{test_eval_fns, test_low_degree};
    use crate::gates::noop::NoopGate;

    #[test]
    fn low_degree() {
        test_low_degree::<GoldilocksField, _, 4>(NoopGate)
    }

    #[test]
    fn eval_fns() -> anyhow::Result<()> {
        test_eval_fns::<GoldilocksField, _, 4>(NoopGate)
    }
}
