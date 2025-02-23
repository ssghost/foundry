use crate::{
    config::{Hardfork, DEFAULT_MNEMONIC},
    eth::pool::transactions::TransactionOrder,
    AccountGenerator, NodeConfig, CHAIN_ID,
};
use anvil_server::ServerConfig;
use clap::Parser;
use ethers::utils::WEI_IN_ETHER;
use std::{
    net::IpAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tracing::log::trace;

#[derive(Clone, Debug, Parser)]
pub struct NodeArgs {
    #[clap(flatten, next_help_heading = "EVM OPTIONS")]
    pub evm_opts: AnvilEvmArgs,

    #[clap(
        long,
        short,
        help = "Port number to listen on.",
        default_value = "8545",
        value_name = "NUM"
    )]
    pub port: u16,

    #[clap(
        long,
        short,
        help = "Number of dev accounts to generate and configure.",
        default_value = "10",
        value_name = "NUM"
    )]
    pub accounts: u64,

    #[clap(
        long,
        help = "The balance of every dev account in Ether.",
        default_value = "10000",
        value_name = "NUM"
    )]
    pub balance: u64,

    #[clap(
        long,
        short,
        help = "BIP39 mnemonic phrase used for generating accounts",
        value_name = "MNEMONIC"
    )]
    pub mnemonic: Option<String>,

    #[clap(
        long,
        help = "Sets the derivation path of the child key to be derived. [default: m/44'/60'/0'/0/]",
        value_name = "DERIVATION_PATH"
    )]
    pub derivation_path: Option<String>,

    #[clap(flatten, next_help_heading = "SERVER OPTIONS")]
    pub server_config: ServerConfig,

    #[clap(long, help = "Don't print anything on startup.")]
    pub silent: bool,

    #[clap(
        long,
        help = "The EVM hardfork to use.",
        default_value = "latest",
        value_name = "HARDFORK"
    )]
    pub hardfork: Hardfork,

    #[clap(
        short,
        long,
        visible_alias = "blockTime",
        help = "Block time in seconds for interval mining.",
        name = "block-time",
        value_name = "SECONDS"
    )]
    pub block_time: Option<u64>,

    #[clap(
        long,
        help = "Writes output of `anvil` as json to user-specified file",
        value_name = "OUT_FILE"
    )]
    pub config_out: Option<String>,

    #[clap(
        long,
        visible_alias = "no-mine",
        help = "Disable auto and interval mining, and mine on demand instead.",
        conflicts_with = "block-time"
    )]
    pub no_mining: bool,

    #[clap(
        long,
        help = "The host the server will listen on",
        value_name = "IP_ADDR",
        env = "ANVIL_IP_ADDR",
        help_heading = "SERVER OPTIONS"
    )]
    pub host: Option<IpAddr>,

    #[clap(
        long,
        help = "How transactions are sorted in the mempool",
        default_value = "fees",
        value_name = "ORDER"
    )]
    pub order: TransactionOrder,
}

impl NodeArgs {
    pub fn into_node_config(self) -> NodeConfig {
        let genesis_balance = WEI_IN_ETHER.saturating_mul(self.balance.into());

        NodeConfig::default()
            .with_gas_limit(self.evm_opts.gas_limit)
            .with_gas_price(self.evm_opts.gas_price)
            .with_hardfork(self.hardfork)
            .with_blocktime(self.block_time.map(std::time::Duration::from_secs))
            .with_no_mining(self.no_mining)
            .with_account_generator(self.account_generator())
            .with_genesis_balance(genesis_balance)
            .with_port(self.port)
            .with_eth_rpc_url(self.evm_opts.fork_url)
            .with_base_fee(self.evm_opts.block_base_fee_per_gas)
            .with_fork_block_number(self.evm_opts.fork_block_number)
            .with_storage_caching(self.evm_opts.no_storage_caching)
            .with_server_config(self.server_config)
            .with_host(self.host)
            .set_silent(self.silent)
            .set_config_out(self.config_out)
            .with_chain_id(self.evm_opts.chain_id.unwrap_or(CHAIN_ID))
            .with_transaction_order(self.order)
    }

    fn account_generator(&self) -> AccountGenerator {
        let mut gen = AccountGenerator::new(self.accounts as usize)
            .phrase(DEFAULT_MNEMONIC)
            .chain_id(self.evm_opts.chain_id.unwrap_or(CHAIN_ID));
        if let Some(ref mnemonic) = self.mnemonic {
            gen = gen.phrase(mnemonic);
        }
        if let Some(ref derivation) = self.derivation_path {
            gen = gen.derivation_path(derivation);
        }
        gen
    }

    /// Starts the node
    ///
    /// See also [crate::spawn()]
    pub async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        let (api, handle) = crate::spawn(self.into_node_config()).await;

        // sets the signal handler to gracefully shutdown.
        let fork = api.get_fork().cloned();
        let running = Arc::new(AtomicUsize::new(0));

        ctrlc::set_handler(move || {
            let prev = running.fetch_add(1, Ordering::SeqCst);
            if prev == 0 {
                // cleaning up and shutting down
                // this will make sure that the fork RPC cache is flushed if caching is configured
                trace!("received shutdown signal, shutting down");
                if let Some(ref fork) = fork {
                    fork.database.read().flush_cache();
                }
                std::process::exit(0);
            }
        })
        .expect("Error setting Ctrl-C handler");

        Ok(handle.await??)
    }
}

// Anvil's evm related arguments
#[derive(Debug, Clone, Parser)]
pub struct AnvilEvmArgs {
    /// Fetch state over a remote endpoint instead of starting from an empty state.
    ///
    /// If you want to fetch state from a specific block number, see --fork-block-number.
    #[clap(
        long,
        short,
        visible_alias = "rpc-url",
        value_name = "URL",
        help_heading = "FORK CONFIG"
    )]
    pub fork_url: Option<String>,

    /// Fetch state from a specific block number over a remote endpoint.
    ///
    /// See --fork-url.
    #[clap(long, requires = "fork-url", value_name = "BLOCK", help_heading = "FORK CONFIG")]
    pub fork_block_number: Option<u64>,

    /// Initial retry backoff on encountering errors.
    ///
    /// See --fork-url.
    #[clap(long, requires = "fork-url", value_name = "BACKOFF", help_heading = "FORK CONFIG")]
    pub fork_retry_backoff: Option<u64>,

    /// Explicitly disables the use of RPC caching.
    ///
    /// All storage slots are read entirely from the endpoint.
    ///
    /// This flag overrides the project's configuration file.
    ///
    /// See --fork-url.
    #[clap(long, requires = "fork-url", help_heading = "FORK CONFIG")]
    pub no_storage_caching: bool,

    /// The block gas limit.
    #[clap(long, value_name = "GAS_LIMIT", help_heading = "ENVIRONMENT CONFIG")]
    pub gas_limit: Option<u64>,

    /// The gas price.
    #[clap(long, value_name = "GAS_PRICE", help_heading = "ENVIRONMENT CONFIG")]
    pub gas_price: Option<u64>,

    /// The base fee in a block.
    #[clap(
        long,
        visible_alias = "base-fee",
        value_name = "FEE",
        help_heading = "ENVIRONMENT CONFIG"
    )]
    pub block_base_fee_per_gas: Option<u64>,

    /// The chain ID.
    #[clap(long, value_name = "CHAIN_ID", help_heading = "ENVIRONMENT CONFIG")]
    pub chain_id: Option<u64>,
}
