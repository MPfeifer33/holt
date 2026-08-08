//! Rotation math — seed to matrix, rotate, unrotate.
//!
//! A space's rotation matrix R ∈ SO(d) is derived from its seed via
//! QR decomposition. The matrix is runtime-only — never persisted.
//!
//! Private spaces derive seeds from `master_password + agent_id` via Argon2id.
//! Shared spaces use random per-space seeds (encrypted at rest by the store).

use std::collections::HashMap;
use std::sync::RwLock;

/// Errors from rotation operations.
#[derive(Debug, thiserror::Error)]
pub enum RotationError {
    #[error("no rotation key for space: {0}")]
    NoKeyForSpace(String),

    #[error("dimension mismatch: matrix is {matrix_dim}d, vector is {vector_dim}d")]
    DimensionMismatch {
        matrix_dim: usize,
        vector_dim: usize,
    },

    #[error("key derivation failed: {0}")]
    DerivationFailed(String),

    #[error("rotation error: {0}")]
    Internal(#[from] anyhow::Error),
}

/// Manages rotation matrices for all active spaces.
///
/// Matrices are computed from seeds at startup and held in memory.
/// Identity spaces (commons) use R=I — no computation or storage needed.
pub struct RotationManager {
    dimension: usize,
    /// space_id → rotation matrix (flattened row-major, d×d).
    /// Identity spaces are not stored — rotate/unrotate are no-ops.
    matrices: RwLock<HashMap<String, Vec<f32>>>,
}

impl RotationManager {
    /// Create a new rotation manager for vectors of the given dimension.
    pub fn new(dimension: usize) -> Self {
        Self {
            dimension,
            matrices: RwLock::new(HashMap::new()),
        }
    }

    /// Derive and cache a rotation matrix from a seed.
    ///
    /// Generates a deterministic d×d Gaussian matrix from the seed,
    /// computes QR decomposition, normalizes to SO(d) (det = +1).
    pub fn load_space(&self, space_id: &str, seed: &[u8; 32]) -> Result<(), RotationError> {
        let matrix = derive_rotation_matrix(seed, self.dimension)?;
        self.matrices
            .write()
            .map_err(|e| RotationError::Internal(anyhow::anyhow!("lock poisoned: {e}")))?
            .insert(space_id.to_string(), matrix);
        Ok(())
    }

    /// Register an identity space (no rotation needed).
    pub fn load_identity_space(&self, space_id: &str) -> Result<(), RotationError> {
        // Identity spaces are simply not in the map — rotate/unrotate check for this.
        // But we track them so has_space works.
        // We store an empty vec as a sentinel for identity.
        self.matrices
            .write()
            .map_err(|e| RotationError::Internal(anyhow::anyhow!("lock poisoned: {e}")))?
            .insert(space_id.to_string(), Vec::new());
        Ok(())
    }

    /// Check if a space has been loaded.
    pub fn has_space(&self, space_id: &str) -> bool {
        self.matrices
            .read()
            .map(|m| m.contains_key(space_id))
            .unwrap_or(false)
    }

    /// Rotate a vector for storage in a space.
    /// For identity spaces, returns the vector unchanged.
    pub fn rotate(&self, space_id: &str, vector: &[f32]) -> Result<Vec<f32>, RotationError> {
        if vector.len() != self.dimension {
            return Err(RotationError::DimensionMismatch {
                matrix_dim: self.dimension,
                vector_dim: vector.len(),
            });
        }

        let matrices = self
            .matrices
            .read()
            .map_err(|e| RotationError::Internal(anyhow::anyhow!("lock poisoned: {e}")))?;

        let matrix = matrices
            .get(space_id)
            .ok_or_else(|| RotationError::NoKeyForSpace(space_id.to_string()))?;

        // Identity space — empty sentinel
        if matrix.is_empty() {
            return Ok(vector.to_vec());
        }

        Ok(matrix_vector_multiply(matrix, vector, self.dimension))
    }

    /// Unrotate a vector (for sharing: unrotate from source space, re-rotate into target).
    /// For identity spaces, returns the vector unchanged.
    /// Unrotation uses the transpose of the rotation matrix (R^T = R^{-1} for orthogonal matrices).
    pub fn unrotate(&self, space_id: &str, vector: &[f32]) -> Result<Vec<f32>, RotationError> {
        if vector.len() != self.dimension {
            return Err(RotationError::DimensionMismatch {
                matrix_dim: self.dimension,
                vector_dim: vector.len(),
            });
        }

        let matrices = self
            .matrices
            .read()
            .map_err(|e| RotationError::Internal(anyhow::anyhow!("lock poisoned: {e}")))?;

        let matrix = matrices
            .get(space_id)
            .ok_or_else(|| RotationError::NoKeyForSpace(space_id.to_string()))?;

        if matrix.is_empty() {
            return Ok(vector.to_vec());
        }

        // Transpose multiply: R^T * v
        Ok(matrix_transpose_vector_multiply(
            matrix,
            vector,
            self.dimension,
        ))
    }

    /// Derive a private space seed from master password + agent ID via Argon2id.
    pub fn derive_private_seed(
        master_password: &[u8],
        agent_id: &str,
    ) -> Result<[u8; 32], RotationError> {
        use argon2::{Algorithm, Argon2, Params, Version};

        let salt = format!("anchor-v1-private-{agent_id}");
        let params = Params::new(65536, 3, 1, Some(32))
            .map_err(|e| RotationError::DerivationFailed(e.to_string()))?;
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

        let mut seed = [0u8; 32];
        argon2
            .hash_password_into(master_password, salt.as_bytes(), &mut seed)
            .map_err(|e| RotationError::DerivationFailed(e.to_string()))?;

        Ok(seed)
    }
}

// ── Internal Math ───────────────────────────────────────────────────

/// Derive a rotation matrix R ∈ SO(d) from a 32-byte seed.
///
/// Algorithm:
/// 1. Expand the seed into d×d deterministic pseudo-random f32 values
///    using BLAKE3 in keyed-hash mode (seed as key, counter as input).
/// 2. Treat as a d×d matrix A and compute QR decomposition: A = Q * R.
/// 3. Normalize Q signs so that the diagonal of R is positive (Householder sign fix).
///    This ensures Q is unique and deterministic for a given A.
/// 4. If det(Q) < 0, negate the last column to produce a proper rotation (det = +1).
///
/// The result is a proper rotation matrix in SO(d): orthogonal and orientation-preserving.
fn derive_rotation_matrix(seed: &[u8; 32], dim: usize) -> Result<Vec<f32>, RotationError> {
    use nalgebra::DMatrix;

    // ── Step 1: Expand seed into d×d pseudo-random matrix ──
    // Use BLAKE3 in keyed-hash mode: seed is the key, sequential counter is the input.
    // Each hash produces 32 bytes = 8 f32 values. We need dim*dim values.
    let hasher_key: [u8; 32] = *seed;
    let total = dim * dim;
    let mut raw = Vec::with_capacity(total);

    let mut counter: u64 = 0;
    while raw.len() < total {
        let mut hasher = blake3::Hasher::new_keyed(&hasher_key);
        hasher.update(&counter.to_le_bytes());
        let hash = hasher.finalize();
        let bytes = hash.as_bytes();

        // Extract f32 values from the hash output (4 bytes each, 8 per hash)
        for chunk in bytes.chunks_exact(4) {
            if raw.len() >= total {
                break;
            }
            // Convert to u32, then map to roughly Gaussian-ish distribution
            // via Box-Muller-lite: uniform [0,1) → subtract 0.5 → scale.
            // For QR decomposition, the exact distribution doesn't matter —
            // any non-degenerate matrix works. Uniform noise is fine.
            let u = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            let val = (u as f64 / u32::MAX as f64) * 2.0 - 1.0; // map to [-1, 1]
            raw.push(val as f32);
        }
        counter += 1;
    }

    // ── Step 2: QR decomposition ──
    // nalgebra stores column-major; DMatrix::from_row_slice handles our row-major data.
    let a = DMatrix::from_row_slice(dim, dim, &raw[..total]);
    let qr = a.qr();
    let mut q = qr.q();
    let r = qr.r();

    // ── Step 3: Householder sign normalization ──
    // QR decomposition is not unique: Q can have sign flips per column.
    // We normalize so R has a positive diagonal, making Q deterministic.
    for i in 0..dim {
        if r[(i, i)] < 0.0 {
            // Negate column i of Q
            for j in 0..dim {
                q[(j, i)] = -q[(j, i)];
            }
        }
    }

    // ── Step 4: Ensure det(Q) = +1 (proper rotation, not reflection) ──
    let det = q.determinant();
    if det < 0.0 {
        // Negate the last column to flip the determinant
        let last = dim - 1;
        for j in 0..dim {
            q[(j, last)] = -q[(j, last)];
        }
    }

    // ── Extract to row-major Vec<f32> ──
    // nalgebra is column-major, so we read row by row.
    let mut matrix = vec![0.0f32; total];
    for i in 0..dim {
        for j in 0..dim {
            matrix[i * dim + j] = q[(i, j)] as f32;
        }
    }

    Ok(matrix)
}

/// Multiply a d×d matrix (row-major) by a d-vector.
fn matrix_vector_multiply(matrix: &[f32], vector: &[f32], dim: usize) -> Vec<f32> {
    let mut result = vec![0.0f32; dim];
    for i in 0..dim {
        let mut sum = 0.0f32;
        let row_start = i * dim;
        for j in 0..dim {
            sum += matrix[row_start + j] * vector[j];
        }
        result[i] = sum;
    }
    result
}

/// Multiply the transpose of a d×d matrix (row-major) by a d-vector.
/// For orthogonal matrices, this is the inverse rotation.
fn matrix_transpose_vector_multiply(matrix: &[f32], vector: &[f32], dim: usize) -> Vec<f32> {
    let mut result = vec![0.0f32; dim];
    for i in 0..dim {
        let mut sum = 0.0f32;
        for j in 0..dim {
            sum += matrix[j * dim + i] * vector[j];
        }
        result[i] = sum;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_space_passthrough() {
        let rm = RotationManager::new(3);
        rm.load_identity_space("commons").unwrap();

        let v = vec![1.0, 2.0, 3.0];
        let rotated = rm.rotate("commons", &v).unwrap();
        assert_eq!(v, rotated);

        let unrotated = rm.unrotate("commons", &v).unwrap();
        assert_eq!(v, unrotated);
    }

    #[test]
    fn dimension_mismatch_rejected() {
        let rm = RotationManager::new(3);
        rm.load_identity_space("test").unwrap();

        let v = vec![1.0, 2.0]; // wrong dimension
        assert!(rm.rotate("test", &v).is_err());
    }

    #[test]
    fn unknown_space_rejected() {
        let rm = RotationManager::new(3);
        let v = vec![1.0, 2.0, 3.0];
        assert!(rm.rotate("nonexistent", &v).is_err());
    }

    #[test]
    fn seed_derivation_deterministic() {
        let seed1 = RotationManager::derive_private_seed(b"password123", "agent-demo").unwrap();
        let seed2 = RotationManager::derive_private_seed(b"password123", "agent-demo").unwrap();
        assert_eq!(seed1, seed2);

        // Different agent → different seed
        let seed3 = RotationManager::derive_private_seed(b"password123", "agent-echo").unwrap();
        assert_ne!(seed1, seed3);
    }

    #[test]
    fn rotation_matrix_is_orthogonal() {
        // An orthogonal matrix satisfies R * R^T = I
        let dim = 8;
        let seed = [42u8; 32];
        let matrix = derive_rotation_matrix(&seed, dim).unwrap();

        // Compute R * R^T
        for i in 0..dim {
            for j in 0..dim {
                let mut dot = 0.0f32;
                for k in 0..dim {
                    dot += matrix[i * dim + k] * matrix[j * dim + k];
                }
                if i == j {
                    assert!(
                        (dot - 1.0).abs() < 1e-5,
                        "diagonal [{i},{j}] should be 1.0, got {dot}"
                    );
                } else {
                    assert!(
                        dot.abs() < 1e-5,
                        "off-diagonal [{i},{j}] should be 0.0, got {dot}"
                    );
                }
            }
        }
    }

    #[test]
    fn rotation_matrix_has_positive_determinant() {
        // det(R) = +1 for a proper rotation (not a reflection)
        use nalgebra::DMatrix;

        let dim = 8;
        let seed = [99u8; 32];
        let flat = derive_rotation_matrix(&seed, dim).unwrap();

        let m = DMatrix::from_row_slice(dim, dim, &flat);
        let det = m.determinant();
        assert!(
            (det - 1.0).abs() < 1e-4,
            "determinant should be +1.0, got {det}"
        );
    }

    #[test]
    fn rotation_matrix_is_deterministic() {
        let dim = 16;
        let seed = [77u8; 32];

        let m1 = derive_rotation_matrix(&seed, dim).unwrap();
        let m2 = derive_rotation_matrix(&seed, dim).unwrap();
        assert_eq!(m1, m2, "same seed must produce identical matrices");
    }

    #[test]
    fn different_seeds_produce_different_matrices() {
        let dim = 8;
        let m1 = derive_rotation_matrix(&[1u8; 32], dim).unwrap();
        let m2 = derive_rotation_matrix(&[2u8; 32], dim).unwrap();
        assert_ne!(m1, m2, "different seeds must produce different matrices");
    }

    #[test]
    fn rotation_is_not_identity() {
        // A real rotation matrix should NOT be the identity
        let dim = 8;
        let seed = [42u8; 32];
        let matrix = derive_rotation_matrix(&seed, dim).unwrap();

        let mut identity = vec![0.0f32; dim * dim];
        for i in 0..dim {
            identity[i * dim + i] = 1.0;
        }

        assert_ne!(
            matrix, identity,
            "rotation matrix must not be identity — that would mean no isolation"
        );
    }

    #[test]
    fn rotate_then_unrotate_roundtrips() {
        let dim = 32; // realistic dimension
        let rm = RotationManager::new(dim);
        let seed = [55u8; 32];
        rm.load_space("test", &seed).unwrap();

        let original: Vec<f32> = (0..dim).map(|i| (i as f32) * 0.1).collect();
        let rotated = rm.rotate("test", &original).unwrap();

        // Rotated vector should be different from original
        assert_ne!(
            original, rotated,
            "rotated vector must differ from original"
        );

        // Unrotating should recover the original
        let recovered = rm.unrotate("test", &rotated).unwrap();
        for (i, (a, b)) in original.iter().zip(recovered.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-4,
                "roundtrip failed at index {i}: expected {a}, got {b}"
            );
        }
    }

    #[test]
    fn different_spaces_produce_different_rotations() {
        let dim = 8;
        let rm = RotationManager::new(dim);
        rm.load_space("space-a", &[1u8; 32]).unwrap();
        rm.load_space("space-b", &[2u8; 32]).unwrap();

        let v = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let rotated_a = rm.rotate("space-a", &v).unwrap();
        let rotated_b = rm.rotate("space-b", &v).unwrap();

        assert_ne!(
            rotated_a, rotated_b,
            "different spaces must produce different rotations"
        );
    }

    #[test]
    fn rotation_preserves_vector_norm() {
        // Orthogonal matrices preserve Euclidean norm
        let dim = 32;
        let rm = RotationManager::new(dim);
        rm.load_space("test", &[42u8; 32]).unwrap();

        let v: Vec<f32> = (0..dim).map(|i| (i as f32 + 1.0) * 0.3).collect();
        let norm_before: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();

        let rotated = rm.rotate("test", &v).unwrap();
        let norm_after: f32 = rotated.iter().map(|x| x * x).sum::<f32>().sqrt();

        assert!(
            (norm_before - norm_after).abs() < 1e-4,
            "rotation must preserve norm: {norm_before} vs {norm_after}"
        );
    }

    #[test]
    fn rotation_at_embedding_dimension() {
        // Test at the actual BGE-Base dimension (768)
        let dim = 768;
        let rm = RotationManager::new(dim);
        rm.load_space("real-space", &[42u8; 32]).unwrap();

        // Create a unit-ish vector
        let mut v = vec![0.0f32; dim];
        v[0] = 1.0;

        let rotated = rm.rotate("real-space", &v).unwrap();
        let recovered = rm.unrotate("real-space", &rotated).unwrap();

        // Verify roundtrip at full dimension
        for (i, (a, b)) in v.iter().zip(recovered.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-3,
                "768d roundtrip failed at index {i}: expected {a}, got {b}"
            );
        }
    }
}
