use rete_core::{Header, Rete, CURRENT_FORMAT_VERSION, HEADER_LEN};

const V1: &[u8] = include_bytes!("fixtures/v1/minimal.rete");

#[test]
fn every_future_reader_opens_the_v1_baseline() {
    let header = Header::from_bytes(&V1[..HEADER_LEN]).unwrap();
    assert_eq!(header.version, CURRENT_FORMAT_VERSION);
    assert_eq!(header.version, 0x05);

    let graph = Rete::open(V1).unwrap();
    assert_eq!(graph.query(None, None, None).len(), 2);
    assert_eq!(graph.graph_names(), vec!["<http://example.test/people>"]);
}
