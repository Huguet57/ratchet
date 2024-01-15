use std::ops::RangeInclusive;

use crate::{DType, RVec, Tensor};

#[derive(Debug, thiserror::Error)]
pub enum InvariantError {
    #[error("Shape mismatch at {left},{right}, {a} != {b}.")]
    ShapeMismatch {
        left: usize,
        right: usize,
        a: usize, //RDim
        b: usize,
    },
    #[error("Rank mismatch. {accepted:?} != {actual}.")]
    RankMismatch {
        accepted: RangeInclusive<usize>,
        actual: usize,
    },
    #[error("Wrong input arity. Allowed range is {accepted:?}, node has {actual}.")]
    InputArity {
        accepted: RangeInclusive<usize>,
        actual: usize,
    },
    #[error("Wrong output arity. Allowed is {accepted:?}, node has {actual}.")]
    OutputArity {
        accepted: RangeInclusive<usize>,
        actual: usize,
    },
    #[error("DType mismatch, expected {expected:?}, got {actual:?}.")]
    DTypeMismatch { expected: DType, actual: DType },
}

///Enforcer is a collection of methods to enforce invariants.
///Used during the inference of operation outputs.
pub struct Enforcer;

//TODO: switch to slices
impl Enforcer {
    pub fn check_input_arity(inputs: &[Tensor], expected: usize) -> Result<(), InvariantError> {
        Self::check_input_arity_range(inputs, expected..=expected + 1)
    }

    pub fn check_input_arity_range(
        inputs: &[Tensor],
        accepted: RangeInclusive<usize>,
    ) -> Result<(), InvariantError> {
        if !accepted.contains(&inputs.len()) {
            Err(InvariantError::InputArity {
                accepted,
                actual: inputs.len(),
            })
        } else {
            Ok(())
        }
    }

    pub fn check_output_arity_range(
        outputs: &[Tensor],
        accepted: RangeInclusive<usize>,
    ) -> Result<(), InvariantError> {
        if !accepted.contains(&outputs.len()) {
            Err(InvariantError::OutputArity {
                accepted,
                actual: outputs.len(),
            })
        } else {
            Ok(())
        }
    }

    pub fn check_output_arity(outputs: &[Tensor], expected: usize) -> Result<(), InvariantError> {
        Self::check_output_arity_range(outputs, expected..=expected + 1)
    }

    pub fn check_shape_pair(
        a: &Tensor,
        b: &Tensor,
        left: usize,
        right: usize,
    ) -> Result<(), InvariantError> {
        if a.shape()[left] != b.shape()[right] {
            return Err(InvariantError::ShapeMismatch {
                left,
                right,
                a: a.shape()[left].clone(),
                b: b.shape()[right].clone(),
            });
        }
        Ok(())
    }

    pub fn match_shapes_at_index(
        tensors: &RVec<Tensor>,
        index: usize,
    ) -> Result<(), InvariantError> {
        let shape = tensors[0].shape();
        for tensor in tensors.iter().skip(1) {
            if shape[index] != tensor.shape()[index] {
                return Err(InvariantError::ShapeMismatch {
                    left: index,
                    right: index,
                    a: shape[index].clone(),
                    b: tensor.shape()[index].clone(),
                });
            }
        }
        Ok(())
    }

    pub fn assert_rank(tensor: &Tensor, rank: usize) -> Result<(), InvariantError> {
        if tensor.rank() != rank {
            return Err(InvariantError::RankMismatch {
                accepted: rank..=rank + 1,
                actual: tensor.rank(),
            });
        }
        Ok(())
    }

    pub fn assert_dtype(tensor: &Tensor, dtype: DType) -> Result<(), InvariantError> {
        if tensor.dt() != dtype {
            return Err(InvariantError::DTypeMismatch {
                expected: dtype,
                actual: tensor.dt(),
            });
        }
        Ok(())
    }

    pub fn assert_rank_range(
        tensor: &Tensor,
        range: RangeInclusive<usize>,
    ) -> Result<(), InvariantError> {
        if !range.contains(&tensor.rank()) {
            return Err(InvariantError::RankMismatch {
                accepted: range,
                actual: tensor.rank(),
            });
        }
        Ok(())
    }

    pub fn assert_equal_ranks(tensors: &RVec<Tensor>) -> Result<usize, InvariantError> {
        let rank = tensors[0].rank();
        for tensor in tensors.iter().skip(1) {
            if rank != tensor.rank() {
                return Err(InvariantError::RankMismatch {
                    accepted: rank..=rank + 1,
                    actual: tensor.rank(),
                });
            }
        }
        Ok(rank)
    }
}