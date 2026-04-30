use tx3_tir::model::v1beta0::{Expression, Param};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExprKind {
    Const,
    ParamDriven,
    Computed,
}

pub fn classify(expr: &Expression) -> ExprKind {
    match expr {
        Expression::None
        | Expression::Bytes(_)
        | Expression::Number(_)
        | Expression::Bool(_)
        | Expression::String(_)
        | Expression::Address(_)
        | Expression::Hash(_)
        | Expression::UtxoRefs(_)
        | Expression::UtxoSet(_) => ExprKind::Const,

        Expression::List(xs) => combine(xs.iter().map(classify)),
        Expression::Map(pairs) => combine(
            pairs
                .iter()
                .flat_map(|(k, v)| [classify(k), classify(v)].into_iter()),
        ),
        Expression::Tuple(boxed) => combine([classify(&boxed.0), classify(&boxed.1)].into_iter()),
        Expression::Struct(s) => combine(s.fields.iter().map(classify)),
        Expression::Assets(assets) => combine(assets.iter().flat_map(|a| {
            [
                classify(&a.policy),
                classify(&a.asset_name),
                classify(&a.amount),
            ]
            .into_iter()
        })),

        Expression::EvalParam(p) => match p.as_ref() {
            Param::Set(inner) => classify(inner),
            Param::ExpectValue(_, _) | Param::ExpectInput(_, _) | Param::ExpectFees => {
                ExprKind::ParamDriven
            }
        },

        Expression::EvalBuiltIn(_) | Expression::EvalCompiler(_) | Expression::EvalCoerce(_) => {
            ExprKind::Computed
        }

        Expression::AdHocDirective(_) => ExprKind::Computed,
    }
}

fn combine(kinds: impl IntoIterator<Item = ExprKind>) -> ExprKind {
    let mut acc = ExprKind::Const;
    for k in kinds {
        acc = match (acc, k) {
            (ExprKind::Const, ExprKind::Const) => ExprKind::Const,
            (_, ExprKind::Computed) | (ExprKind::Computed, _) => ExprKind::Computed,
            _ => ExprKind::ParamDriven,
        };
    }
    acc
}

/// If `expr` is a `Const`-classified `Expression::Address`, return its bytes.
pub fn const_address(expr: &Expression) -> Option<&[u8]> {
    match expr {
        Expression::Address(bytes) => Some(bytes),
        Expression::EvalParam(p) => match p.as_ref() {
            Param::Set(inner) => const_address(inner),
            _ => None,
        },
        _ => None,
    }
}

/// If `expr` is a `Const`-classified `Expression::UtxoRefs`, return its refs.
pub fn const_utxo_refs(expr: &Expression) -> Option<&[tx3_tir::model::core::UtxoRef]> {
    match expr {
        Expression::UtxoRefs(refs) => Some(refs),
        Expression::EvalParam(p) => match p.as_ref() {
            Param::Set(inner) => const_utxo_refs(inner),
            _ => None,
        },
        _ => None,
    }
}

/// If `expr` is a `Const`-classified number, return its value.
pub fn const_number(expr: &Expression) -> Option<i128> {
    match expr {
        Expression::Number(n) => Some(*n),
        Expression::EvalParam(p) => match p.as_ref() {
            Param::Set(inner) => const_number(inner),
            _ => None,
        },
        _ => None,
    }
}

/// If `expr` is a `Const`-classified bytes/hash, return its bytes.
pub fn const_bytes(expr: &Expression) -> Option<&[u8]> {
    match expr {
        Expression::Bytes(b) | Expression::Hash(b) => Some(b),
        Expression::EvalParam(p) => match p.as_ref() {
            Param::Set(inner) => const_bytes(inner),
            _ => None,
        },
        _ => None,
    }
}

/// Walk an `Assets` expression and return the const policy bytes encountered.
pub fn const_policies_in(expr: &Expression) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    collect_const_policies(expr, &mut out);
    out
}

fn collect_const_policies(expr: &Expression, out: &mut Vec<Vec<u8>>) {
    match expr {
        Expression::Assets(assets) => {
            for a in assets {
                if let Some(p) = const_bytes(&a.policy) {
                    if !p.is_empty() {
                        out.push(p.to_vec());
                    }
                }
            }
        }
        Expression::List(xs) => xs.iter().for_each(|e| collect_const_policies(e, out)),
        Expression::EvalParam(p) => {
            if let Param::Set(inner) = p.as_ref() {
                collect_const_policies(inner, out);
            }
        }
        _ => {}
    }
}
