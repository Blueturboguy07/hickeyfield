fn main() {
    for m in halation_core::catalog::parse(halation_core::catalog::VENDORED_SPEC) {
        println!("{}\t{}\t{}", m.modality, m.id, m.display_name);
    }
}
