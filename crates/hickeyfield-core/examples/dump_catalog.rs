fn main() {
    for m in hickeyfield_core::catalog::parse(hickeyfield_core::catalog::VENDORED_SPEC) {
        println!("{}\t{}\t{}", m.modality, m.id, m.display_name);
    }
}
