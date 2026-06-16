use serde_bytes::ByteBuf;

#[test]
fn output_addresses_with_datum_populated_correctly() {
    let bytes = hex::decode(include_str!("fixtures/sp_deposit_71e89010.cbor.hex").trim()).unwrap();
    let payload = tx3_lift_cardano::payload::CardanoPayload::from_cbor(bytes).unwrap();
    let summary = tx3_lift_cardano::summarize::summarize(&payload).unwrap();

    // Total distinct output addresses (3 outputs, but the script address appears twice).
    assert_eq!(
        summary.output_addresses.len(),
        3,
        "expected 3 distinct output addresses"
    );

    // Exactly 2 of those carry a datum.
    assert_eq!(
        summary.output_addresses_with_datum.len(),
        2,
        "expected 2 output addresses with datum"
    );

    // Script address (appears in two outputs, both with datum).
    let script_addr = ByteBuf::from(
        hex::decode("311c53ed6f616687b340ac83072ec65a9787583c01d6bae0314e1d61d0b8358aadd30c60eba168608ad5e875592e9b7cb8c700827cde87f9a3")
            .unwrap(),
    );
    assert!(
        summary.output_addresses_with_datum.contains(&script_addr),
        "script address must be in output_addresses_with_datum"
    );

    // Payment address that also carries a datum.
    let payment_addr_with_datum = ByteBuf::from(
        hex::decode("011ff8ec747a4655f4f3abfe66233fb0343954025143e9134ca779640deb28ab85fa3384a04ef6778e5816fdb3412c6c2a9956cedd011f0dbe")
            .unwrap(),
    );
    assert!(
        summary
            .output_addresses_with_datum
            .contains(&payment_addr_with_datum),
        "payment address with datum must be in output_addresses_with_datum"
    );

    // Change address (no datum) must NOT appear in the datum set.
    let change_addr = ByteBuf::from(
        hex::decode("01d31ae59bac6318cbf598a2b417ebdb5092f16b31472856fffff5e4777aa4bc5d02917c227014f9ed0d16cf096b0aa8fdc4aa3ddb374f98ce")
            .unwrap(),
    );
    assert!(
        !summary.output_addresses_with_datum.contains(&change_addr),
        "change address (no datum) must not be in output_addresses_with_datum"
    );

    // output_addresses_with_datum is a strict subset of output_addresses.
    assert!(
        summary
            .output_addresses_with_datum
            .is_subset(&summary.output_addresses),
        "output_addresses_with_datum must be a subset of output_addresses"
    );
    assert!(
        summary.output_addresses_with_datum.len() < summary.output_addresses.len(),
        "output_addresses_with_datum must be a STRICT subset (smaller)"
    );
}
