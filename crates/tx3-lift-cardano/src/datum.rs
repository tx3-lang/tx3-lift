use pallas::ledger::primitives::{BigInt, Int, MaybeIndefArray, PlutusData};
use tx3_tir::model::v1beta0::{Expression, StructExpr};

pub fn plutus_data_to_expression(data: &PlutusData) -> Expression {
    match data {
        PlutusData::BoundedBytes(b) => Expression::Bytes(b.to_vec()),
        PlutusData::BigInt(bi) => big_int_to_expr(bi),
        PlutusData::Constr(c) => {
            let constructor = c.any_constructor.unwrap_or(c.tag) as usize;
            let fields = match &c.fields {
                MaybeIndefArray::Def(items) => items.iter().map(plutus_data_to_expression).collect(),
                MaybeIndefArray::Indef(items) => {
                    items.iter().map(plutus_data_to_expression).collect()
                }
            };
            Expression::Struct(StructExpr {
                constructor,
                fields,
            })
        }
        PlutusData::Array(items) => {
            let xs = match items {
                MaybeIndefArray::Def(v) => v.iter().map(plutus_data_to_expression).collect(),
                MaybeIndefArray::Indef(v) => v.iter().map(plutus_data_to_expression).collect(),
            };
            Expression::List(xs)
        }
        PlutusData::Map(pairs) => {
            let entries = pairs
                .iter()
                .map(|(k, v)| (plutus_data_to_expression(k), plutus_data_to_expression(v)))
                .collect();
            Expression::Map(entries)
        }
    }
}

fn big_int_to_expr(bi: &BigInt) -> Expression {
    match bi {
        BigInt::Int(Int(int)) => match i128::try_from(*int) {
            Ok(n) => Expression::Number(n),
            Err(_) => Expression::Bytes(Vec::new()),
        },
        BigInt::BigUInt(bytes) | BigInt::BigNInt(bytes) => Expression::Bytes(bytes.to_vec()),
    }
}
