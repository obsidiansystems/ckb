use crate::error::RPCError;
use ckb_chain::{chain::ChainController, switch::Switch};
use ckb_jsonrpc_types::{Block, BlockView, Cycle, Transaction};
use ckb_logger::error;
use ckb_network::{NetworkController, SupportProtocols};
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_types::{core, packed, prelude::*, H256};
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use std::collections::HashSet;
use std::sync::Arc;

#[rpc(server)]
pub trait IntegrationTestRpc {
    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"add_node","params": ["QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS", "/ip4/192.168.2.100/tcp/30002"]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "add_node")]
    fn add_node(&self, peer_id: String, address: String) -> Result<()>;

    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"remove_node","params": ["QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS"]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "remove_node")]
    fn remove_node(&self, peer_id: String) -> Result<()>;

    #[rpc(name = "process_block_without_verify")]
    fn process_block_without_verify(&self, data: Block, broadcast: bool) -> Result<Option<H256>>;

    #[rpc(name = "truncate")]
    fn truncate(&self, target_tip_hash: H256) -> Result<()>;

    #[rpc(name = "broadcast_transaction")]
    fn broadcast_transaction(&self, transaction: Transaction, cycles: Cycle) -> Result<H256>;

    #[rpc(name = "get_fork_block")]
    fn get_fork_block(&self, _hash: H256) -> Result<Option<BlockView>>;
}

pub(crate) struct IntegrationTestRpcImpl {
    pub network_controller: NetworkController,
    pub shared: Shared,
    pub chain: ChainController,
}

impl IntegrationTestRpc for IntegrationTestRpcImpl {
    fn add_node(&self, peer_id: String, address: String) -> Result<()> {
        self.network_controller.add_node(
            &peer_id.parse().expect("invalid peer_id"),
            address.parse().expect("invalid address"),
        );
        Ok(())
    }

    fn remove_node(&self, peer_id: String) -> Result<()> {
        self.network_controller
            .remove_node(&peer_id.parse().expect("invalid peer_id"));
        Ok(())
    }

    fn process_block_without_verify(&self, data: Block, broadcast: bool) -> Result<Option<H256>> {
        let block: packed::Block = data.into();
        let block: Arc<core::BlockView> = Arc::new(block.into_view());
        let ret = self
            .chain
            .internal_process_block(Arc::clone(&block), Switch::DISABLE_ALL);

        if broadcast {
            let content = packed::CompactBlock::build_from_block(&block, &HashSet::new());
            let message = packed::RelayMessage::new_builder().set(content).build();
            if let Err(err) = self
                .network_controller
                .quick_broadcast(SupportProtocols::Relay.protocol_id(), message.as_bytes())
            {
                error!("Broadcast new block failed: {:?}", err);
            }
        }
        if ret.is_ok() {
            Ok(Some(block.hash().unpack()))
        } else {
            error!("process_block_without_verify error: {:?}", ret);
            Ok(None)
        }
    }

    fn truncate(&self, target_tip_hash: H256) -> Result<()> {
        let header = {
            let snapshot = self.shared.snapshot();
            let header = snapshot
                .get_block_header(&target_tip_hash.pack())
                .ok_or_else(|| {
                    RPCError::custom(RPCError::Invalid, "block not found".to_string())
                })?;
            if !snapshot.is_main_chain(&header.hash()) {
                return Err(RPCError::custom(
                    RPCError::Invalid,
                    "block not on main chain".to_string(),
                ));
            }
            header
        };

        // Truncate the chain and database
        self.chain
            .truncate(header.hash())
            .map_err(|err| RPCError::custom(RPCError::Invalid, err.to_string()))?;

        // Clear the tx_pool
        let tx_pool = self.shared.tx_pool_controller();
        tx_pool
            .clear_pool()
            .map_err(|err| RPCError::custom(RPCError::Invalid, err.to_string()))?;

        Ok(())
    }

    fn broadcast_transaction(&self, transaction: Transaction, cycles: Cycle) -> Result<H256> {
        let tx: packed::Transaction = transaction.into();
        let hash = tx.calc_tx_hash();
        let relay_tx = packed::RelayTransaction::new_builder()
            .cycles(cycles.value().pack())
            .transaction(tx)
            .build();
        let relay_txs = packed::RelayTransactions::new_builder()
            .transactions(vec![relay_tx].pack())
            .build();
        let message = packed::RelayMessage::new_builder().set(relay_txs).build();

        if let Err(err) = self
            .network_controller
            .broadcast(SupportProtocols::Relay.protocol_id(), message.as_bytes())
        {
            error!("Broadcast transaction failed: {:?}", err);
            Err(RPCError::custom(RPCError::Invalid, err.to_string()))
        } else {
            Ok(hash.unpack())
        }
    }

    fn get_fork_block(&self, hash: H256) -> Result<Option<BlockView>> {
        let snapshot = self.shared.snapshot();
        if snapshot.is_main_chain(&hash.pack()) {
            return Ok(None);
        }

        Ok(snapshot.get_block(&hash.pack()).map(Into::into))
    }
}
