use parity_scale_codec::{Decode, Encode};
use sc_consensus_pow::{Error, PowAlgorithm};

use sp_api::ProvideRuntimeApi;
use sp_consensus_pow::{DifficultyApi, Seal as RawSeal};

use ethereum_types::{self, U256 as EU256, H256 as EH256};
use sp_core::{U256, H256};
use sp_runtime::generic::BlockId;
use sp_runtime::traits::Block as BlockT;
use std::sync::Arc;
use ethash::{self, quick_get_difficulty, slow_hash_block_number, EthashManager};
use crate::types::{WorkSeal};
use crate::rpc::{error::{Error as EthError}};

/// Determine whether the given hash satisfies the given difficulty.
/// The test is done by multiplying the two together. If the product
/// overflows the bounds of U256, then the product (and thus the hash)
/// was too high.
pub fn hash_meets_difficulty(hash: &H256, difficulty: U256) -> bool {
	let num_hash = U256::from(&hash[..]);
	let (_, overflowed) = num_hash.overflowing_mul(difficulty);

	!overflowed
}

/// A Seal struct that will be encoded to a Vec<u8> as used as the
/// `RawSeal` type.
#[derive(Clone, PartialEq, Eq, Encode, Decode, Debug)]
pub struct Seal {
	pub difficulty: U256,
	pub work: H256,
	pub nonce: U256,
}

/// A not-yet-computed attempt to solve the proof of work. Calling the
/// compute method will compute the hash and return the seal.
#[derive(Clone, PartialEq, Eq, Encode, Decode, Debug)]
pub struct Compute {
	pub difficulty: U256,
	pub pre_hash: H256,
	pub nonce: U256,
}

impl Compute {
	pub fn compute(self) -> Seal {
		let work = H256::from_slice(&self.encode()[..]);

		Seal {
			nonce: self.nonce,
			difficulty: self.difficulty,
			work,
		}
	}
}

/// A minimal PoW algorithm that uses Sha3 hashing.
/// Difficulty is fixed at 1_000_000
#[derive(Clone)]
pub struct MinimalEthashAlgorithm {
	pow: Arc<EthashManager>,
}

impl MinimalEthashAlgorithm {
	pub fn new() -> Self {
		use tempdir::TempDir;

		let tempdir = TempDir::new("").unwrap();
		Self { pow: Arc::new(EthashManager::new(tempdir.path(), None, u64::max_value())), }
	}

	fn verify_seal(&self, seal: &WorkSeal) -> Result<(), EthError> {
		let mut tmp:[u8; 32] = seal.pow_hash.into();
		let pre_hash = EH256::from(tmp);
		tmp = seal.mix_digest.into();
		let mix_digest = EH256::from(tmp);

        let result = self.pow.compute_light(
            seal.header_nr,
            &pre_hash.0,
            seal.nonce,
        );
        let mix = EH256(result.mix_hash);
        let difficulty = ethash::boundary_to_difficulty(&EH256(result.value));
        // println!("******miner", "num: {num}, seed: {seed}, h: {h}, non: {non}, mix: {mix}, res: {res}",
		// 	   num = seal.header_nr,
		// 	   seed = EH256(slow_hash_block_number(seal.header_nr)),
		// 	   h = pre_hash,
		// 	   non = seal.nonce,
		// 	   mix = EH256(result.mix_hash),
		// 	   res = EH256(result.value));

        if mix != mix_digest {
            return Err(EthError::MismatchedH256SealElement);
        }

		// tmp = self.difficulty(seal.pow_hash.into()).unwrap().into();
		// let header_dif = EU256::from(tmp);
        // if difficulty < header_dif {
        //     return Err(EthError::InvalidProofOfWork);
        // }

		// println!("******miner verified ok");
        Ok(())
    }
}

// Here we implement the general PowAlgorithm trait for our concrete EthashAlgorithm
impl<B: BlockT<Hash = H256>> PowAlgorithm<B> for MinimalEthashAlgorithm {
	type Difficulty = U256;

	fn difficulty(&self, _parent: B::Hash) -> Result<Self::Difficulty, Error<B>> {
		// Fixed difficulty hardcoded here
		Ok(U256::from(1_000_000))
	}

	fn verify(
		&self,
		_parent: &BlockId<B>,
		pre_hash: &H256,
		_pre_digest: Option<&[u8]>,
		seal: &RawSeal,
		difficulty: Self::Difficulty,
	) -> Result<bool, Error<B>> {
		// Try to construct a seal object by decoding the raw seal given
		let seal = match WorkSeal::decode(&mut &seal[..]) {
			Ok(seal) => seal,
			Err(_) => return Ok(false),
		};

		let ret = match self.verify_seal(&seal) {
			Ok(_) => return Ok(true),
			Err(_) => return Ok(false),
		};

		Ok(true)
	}
}

/// A complete PoW Algorithm that uses Sha3 hashing.
/// Needs a reference to the client so it can grab the difficulty from the runtime.
pub struct EthashAlgorithm<C> {
	client: Arc<C>,
}

impl<C> EthashAlgorithm<C> {
	pub fn new(client: Arc<C>) -> Self {
		Self { client }
	}
}

// Manually implement clone. Deriving doesn't work because
// it'll derive impl<C: Clone> Clone for EthashAlgorithm<C>. But C in practice isn't Clone.
impl<C> Clone for EthashAlgorithm<C> {
	fn clone(&self) -> Self {
		Self::new(self.client.clone())
	}
}

// Here we implement the general PowAlgorithm trait for our concrete EthashAlgorithm
impl<B: BlockT<Hash = H256>, C> PowAlgorithm<B> for EthashAlgorithm<C>
where
	C: ProvideRuntimeApi<B>,
	C::Api: DifficultyApi<B, U256>,
{
	type Difficulty = U256;

	fn difficulty(&self, parent: B::Hash) -> Result<Self::Difficulty, Error<B>> {
		let parent_id = BlockId::<B>::hash(parent);
		self.client
			.runtime_api()
			.difficulty(&parent_id)
			.map_err(|err| {
				sc_consensus_pow::Error::Environment(format!(
					"Fetching difficulty from runtime failed: {:?}",
					err
				))
			})
	}

	fn verify(
		&self,
		_parent: &BlockId<B>,
		pre_hash: &H256,
		_pre_digest: Option<&[u8]>,
		seal: &RawSeal,
		difficulty: Self::Difficulty,
	) -> Result<bool, Error<B>> {
		// Try to construct a seal object by decoding the raw seal given
		let seal = match Seal::decode(&mut &seal[..]) {
			Ok(seal) => seal,
			Err(_) => return Ok(false),
		};

		// See whether the hash meets the difficulty requirement. If not, fail fast.
		if !hash_meets_difficulty(&seal.work, difficulty) {
			return Ok(false);
		}

		// Make sure the provided work actually comes from the correct pre_hash
		let compute = Compute {
			difficulty,
			pre_hash: *pre_hash,
			nonce: seal.nonce,
		};

		if compute.compute() != seal {
			return Ok(false);
		}

		Ok(true)
	}
}
