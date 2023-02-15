use sdk::{
    Body, Deposit, DepositInput, Header, MainState, RefundInput, Sha256Hash, SideState, Uint256,
    Unlockable, Withdrawal,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

fn main() {
    let main_state = MainState::<MinimalAddress>::default();
    let minimal_state = MinimalState::default();
    dbg!(&main_state, minimal_state);
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
struct Output {
    amount: u64,
    address: MinimalAddress,
}

#[derive(Default, Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
struct MinimalAddress([u8; 32]);

impl Unlockable for MinimalAddress {
    type Signature = String;

    fn check_signature(&self, signature: &Self::Signature) -> bool {
        signature.hash() == self.0
    }
}

type MinimalSignature = <MinimalAddress as Unlockable>::Signature;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
enum Outpoint {
    Coinbase { block_hash: Uint256, n: usize },
    Regular { txid: Uint256, n: usize },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MinimalInput {
    outpoint: Outpoint,
    signature: MinimalSignature,
}

type MinimalDepositInput = DepositInput<MinimalSignature>;
type MinimalRefundInput = RefundInput<MinimalSignature>;
type MinimalWithdrawal = Withdrawal<MinimalAddress>;
type MinimalDeposit = Deposit<MinimalAddress>;

#[derive(Debug, Serialize, Deserialize)]
struct Transaction {
    deposit_inputs: Vec<MinimalDepositInput>,
    refund_inputs: Vec<MinimalRefundInput>,
    inputs: Vec<MinimalInput>,

    withdrawals: Vec<MinimalWithdrawal>,
    outputs: Vec<Output>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct MinimalBody {
    coinbase: Vec<Output>,
    transactions: Vec<Transaction>,
}

impl MinimalBody {
    fn outputs(&self, header: &MinimalHeader) -> HashMap<Outpoint, Output> {
        let block_hash = header.hash();
        let mut outputs: HashMap<Outpoint, Output> = HashMap::new();
        let coinbase_outputs = self
            .coinbase
            .iter()
            .enumerate()
            .map(|(n, output)| (Outpoint::Coinbase { block_hash, n }, output.clone()));
        outputs.extend(coinbase_outputs);
        let regular_outputs = self.transactions.iter().flat_map(|tx| {
            tx.outputs
                .iter()
                .enumerate()
                .map(|(n, output)| (Outpoint::Regular { txid: tx.hash(), n }, output.clone()))
        });
        outputs.extend(regular_outputs);
        outputs
    }

    fn inputs(&self) -> Vec<MinimalInput> {
        self.transactions
            .iter()
            .flat_map(|tx| tx.inputs.clone())
            .collect()
    }
}

type MinimalHeader = Header<<MinimalBody as Body<MinimalAddress>>::Digest>;

impl Body<MinimalAddress> for MinimalBody {
    type Digest = Uint256;

    fn digest(&self) -> Self::Digest {
        self.hash()
    }

    fn withdrawals(&self) -> Vec<Withdrawal<MinimalAddress>> {
        self.transactions
            .iter()
            .flat_map(|tx| tx.withdrawals.clone())
            .collect()
    }

    fn deposit_inputs(&self) -> Vec<DepositInput<MinimalSignature>> {
        self.transactions
            .iter()
            .flat_map(|tx| tx.deposit_inputs.clone())
            .collect()
    }

    fn refund_inputs(&self) -> Vec<RefundInput<MinimalSignature>> {
        self.transactions
            .iter()
            .flat_map(|tx| tx.refund_inputs.clone())
            .collect()
    }
}

#[derive(Debug, Default)]
struct MinimalState {
    utxos: HashSet<Outpoint>,
    outputs: HashMap<Outpoint, Output>,
}

impl SideState<MinimalAddress, MinimalBody> for MinimalState {
    type Error = ();

    fn validate_block(
        &self,
        main_state: &MainState<MinimalAddress>,
        header: &MinimalHeader,
        body: &MinimalBody,
    ) -> bool {
        let inputs = body.inputs();
        let deposit_inputs = body.deposit_inputs();
        let refund_inputs = body.refund_inputs();

        let spent_outputs: Option<Vec<Output>> = inputs
            .iter()
            .map(|input| self.outputs.get(&input.outpoint).cloned())
            .collect();
        let claimed_deposits: Option<Vec<MinimalDeposit>> = deposit_inputs
            .iter()
            .map(|input| main_state.get_deposit(&input.outpoint))
            .collect();
        let refunded_withdrawals: Option<Vec<MinimalWithdrawal>> = refund_inputs
            .iter()
            .map(|input| main_state.get_withdrawal(&input.outpoint))
            .collect();

        let (spent_outputs, claimed_deposits, refunded_withdrawals) =
            match (spent_outputs, claimed_deposits, refunded_withdrawals) {
                (Some(so), Some(cd), Some(rw)) => (so, cd, rw),
                _ => return false,
            };

        let all_signatures_valid = {
            let input_signatures_valid = inputs
                .iter()
                .zip(&spent_outputs)
                .all(|(input, output)| output.address.check_signature(&input.signature));
            let deposit_signatures_valid = deposit_inputs
                .iter()
                .zip(&claimed_deposits)
                .all(|(input, output)| output.address().check_signature(&input.signature));
            let refund_signatures_valid = refund_inputs
                .iter()
                .zip(&refunded_withdrawals)
                .all(|(input, output)| output.address().check_signature(&input.signature));

            input_signatures_valid && deposit_signatures_valid && refund_signatures_valid
        };
        if !all_signatures_valid {
            return false;
        }

        let total_input_amount = {
            let spent_outputs_amount: u64 = spent_outputs.iter().map(|output| output.amount).sum();
            let deposits_amount: u64 = claimed_deposits.iter().map(|output| output.amount()).sum();
            let refunds_amount: u64 = refunded_withdrawals
                .iter()
                .map(|output| output.amount())
                .sum();

            spent_outputs_amount + deposits_amount + refunds_amount
        };
        let total_output_amount = {
            let outputs = body.outputs(header);
            let withdrawals = body.withdrawals();

            let outputs_amount: u64 = outputs.values().map(|output| output.amount).sum();
            let withdrawals_amount: u64 = withdrawals.iter().map(|output| output.amount()).sum();

            outputs_amount + withdrawals_amount
        };
        let total_coinbase_amount: u64 = body.coinbase.iter().map(|output| output.amount).sum();

        if total_coinbase_amount != total_output_amount - total_input_amount {
            return false;
        }
        true
    }

    fn connect(&mut self, header: &MinimalHeader, body: &MinimalBody) -> Result<(), Self::Error> {
        for input in body.inputs() {
            self.utxos.remove(&input.outpoint);
        }
        let outputs = body.outputs(header);
        self.utxos.extend(outputs.keys().cloned());
        self.outputs.extend(outputs);
        Ok(())
    }

    fn disconnect(
        &mut self,
        header: &MinimalHeader,
        body: &MinimalBody,
    ) -> Result<(), Self::Error> {
        let inputs = body.inputs();
        let spent_outpoints = inputs.iter().map(|input| input.outpoint.clone());
        self.utxos.extend(spent_outpoints);
        let outputs = body.outputs(header);
        for utxo in outputs.keys() {
            self.utxos.remove(utxo);
        }
        Ok(())
    }
}

// #[derive(Debug, Serialize, Deserialize)]
// struct Transaction {
//     deposit_inputs: Vec<MinimalDepositInput>,
//     refund_inputs: Vec<MinimalRefundInput>,
//     inputs: Vec<MinimalInput>,

//     withdrawals: Vec<MinimalWithdrawal>,
//     outputs: Vec<Output>,
// }

// #[derive(Debug, Default, Serialize, Deserialize)]
// struct MinimalBody {
//     coinbase: Vec<Output>,
//     transactions: Vec<Transaction>,
// }
