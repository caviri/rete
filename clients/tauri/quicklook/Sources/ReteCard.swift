// ReteCard.swift — read a .rete file's self-description without opening it.
//
// A `.rete` is a whole RDF graph in one file, but learning what it *is* costs
// two reads: the fixed 1 KB header, then the metadata section it points at. No
// dictionary, no index, no query engine. That is what makes a Quick Look
// preview honest here — a 17 GB graph previews exactly as fast as a 200 KB one,
// because the work does not scale with the file.
//
// Mirrors crates/rete-core/src/header.rs (little-endian on disk) and the JS
// reader in experiments/rete-file-explorer/js/rete-fs.js.

import Foundation

/// One entry of the header's typed section directory.
struct ReteSection {
    let kind: UInt16
    let offset: UInt64
    let length: UInt64

    /// SectionKind, from header.rs.
    var name: String {
        switch kind {
        case 1: return "METADATA"
        case 2: return "DICTIONARY"
        case 3: return "INDEX"
        case 4: return "PYRAMID META"
        case 5: return "NAMED GRAPHS"
        case 6: return "TEXT INDEX"
        default: return "UNKNOWN (\(kind))"
        }
    }
}

struct ReteHeader {
    let version: UInt8
    let quadCount: UInt64
    let termCount: UInt64
    let contentHash: String
    let sections: [ReteSection]

    var metadata: ReteSection? { sections.first { $0.kind == 1 } }
}

enum ReteError: Error, LocalizedError {
    case tooSmall(Int)
    case badMagic

    var errorDescription: String? {
        switch self {
        case .tooSmall(let n): return "File is only \(n) bytes — too small to be a .rete."
        case .badMagic: return "Not a .rete file (the RETE magic is missing)."
        }
    }
}

private let HEADER_LEN = 1024
private let SECTION_DIR_OFFSET = 64
private let SECTION_ENTRY_LEN = 24

private extension Data {
    func u16(_ o: Int) -> UInt16 {
        UInt16(self[o]) | (UInt16(self[o + 1]) << 8)
    }
    func u64(_ o: Int) -> UInt64 {
        var v: UInt64 = 0
        for i in (0..<8).reversed() { v = (v << 8) | UInt64(self[o + i]) }
        return v
    }
}

func parseReteHeader(_ head: Data) throws -> ReteHeader {
    guard head.count >= HEADER_LEN else { throw ReteError.tooSmall(head.count) }
    guard head[0] == 0x52, head[1] == 0x45, head[2] == 0x54, head[3] == 0x45 else {
        throw ReteError.badMagic
    }

    let count = Int(head.u16(44))
    var sections: [ReteSection] = []
    for i in 0..<count {
        let p = SECTION_DIR_OFFSET + i * SECTION_ENTRY_LEN
        guard p + SECTION_ENTRY_LEN <= head.count else { break }
        sections.append(ReteSection(kind: head.u16(p), offset: head.u64(p + 8), length: head.u64(p + 16)))
    }

    let hash = (8..<24).map { String(format: "%02x", head[$0]) }.joined()
    return ReteHeader(
        version: head[4],
        quadCount: head.u64(24),
        termCount: head.u64(32),
        contentHash: hash,
        sections: sections
    )
}

/// Header + Dataset Card, read positionally. Never loads the file.
func readReteCard(at url: URL) throws -> (header: ReteHeader, card: [String: Any]?, size: UInt64) {
    let handle = try FileHandle(forReadingFrom: url)
    defer { try? handle.close() }

    let size = try handle.seekToEnd()
    try handle.seek(toOffset: 0)
    let head = try handle.read(upToCount: HEADER_LEN) ?? Data()
    let header = try parseReteHeader(head)

    var card: [String: Any]?
    if let meta = header.metadata, meta.length > 0, meta.offset + meta.length <= size {
        try handle.seek(toOffset: meta.offset)
        if let raw = try handle.read(upToCount: Int(meta.length)),
           let parsed = try? JSONSerialization.jsonObject(with: raw) as? [String: Any] {
            card = parsed
        }
    }
    return (header, card, size)
}

// ------------------------------------------------------------------ display

func humanBytes(_ n: UInt64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"]
    var v = Double(n), i = 0
    while v >= 1024 && i < units.count - 1 { v /= 1024; i += 1 }
    return i == 0 ? "\(n) B" : String(format: "%.2f %@", v, units[i])
}

func humanCount(_ n: UInt64) -> String {
    let f = NumberFormatter()
    f.numberStyle = .decimal
    return f.string(from: NSNumber(value: n)) ?? "\(n)"
}

/// Pull a human string out of whatever shape a card field happens to be.
func cardString(_ value: Any?) -> String? {
    switch value {
    case let s as String: return s.isEmpty ? nil : s
    case let n as NSNumber: return n.stringValue
    case let a as [Any]:
        let parts = a.compactMap { cardString($0) }
        return parts.isEmpty ? nil : parts.joined(separator: ", ")
    case let d as [String: Any]:
        // Common shapes: {"name": …} or {"@id": …}
        return cardString(d["name"] ?? d["title"] ?? d["@id"] ?? d["label"])
    default: return nil
    }
}
