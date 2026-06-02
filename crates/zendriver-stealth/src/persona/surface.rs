//! Surface + Strategy (filled in Task B1).
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Strategy {
    Native,
    Seeded,
    Random,
    Block,
    Value,
}
