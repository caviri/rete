// PreviewViewController.swift — the Quick Look preview for a .rete file.
//
// Press space on a .rete in Finder and this renders its Dataset Card: what the
// graph is, who made it, what licence it carries, how big it is, and which
// sections it holds. Built with AppKit text rather than a WKWebView — a web view
// inside a Quick Look extension brings a second sandbox and an async load for
// what is fundamentally a page of static text.

import AppKit
import Quartz

class PreviewViewController: NSViewController, QLPreviewingController {

    private let scroll = NSScrollView()
    private let text = NSTextView()

    override func loadView() {
        let root = NSView(frame: NSRect(x: 0, y: 0, width: 680, height: 520))

        text.isEditable = false
        text.isSelectable = true
        text.drawsBackground = false
        text.textContainerInset = NSSize(width: 22, height: 20)
        text.autoresizingMask = [.width]
        text.isVerticallyResizable = true
        text.isHorizontallyResizable = false
        text.textContainer?.widthTracksTextView = true

        scroll.documentView = text
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.autoresizingMask = [.width, .height]
        scroll.frame = root.bounds

        root.addSubview(scroll)
        view = root
    }

    func preparePreviewOfFile(at url: URL, completionHandler handler: @escaping (Error?) -> Void) {
        do {
            let (header, card, size) = try readReteCard(at: url)
            text.textStorage?.setAttributedString(render(url: url, header: header, card: card, size: size))
            handler(nil)
        } catch {
            // Hand the error back so Finder falls through to its generic
            // preview rather than showing an empty pane.
            handler(error)
        }
    }

    // ----------------------------------------------------------- rendering

    private func render(url: URL, header: ReteHeader, card: [String: Any]?, size: UInt64) -> NSAttributedString {
        let out = NSMutableAttributedString()

        let title = cardString(card?["title"]) ?? url.lastPathComponent
        out.append(line(title, size: 22, weight: .semibold, spacingAfter: 2))

        let subtitle = "\(humanBytes(size)) · \(humanCount(header.quadCount)) quads · \(humanCount(header.termCount)) terms"
        out.append(line(subtitle, size: 11, color: .secondaryLabelColor, mono: true, spacingAfter: 14))

        if let description = cardString(card?["description"]) {
            out.append(line(description, size: 13, color: .labelColor, spacingAfter: 16))
        }

        // The fields worth surfacing, in the order someone actually asks them.
        let facts: [(String, String?)] = [
            ("Licence", cardString(card?["license"] ?? card?["licence"])),
            ("Publisher", cardString(card?["publisher"] ?? card?["creator"] ?? card?["author"])),
            ("Source", cardString(card?["source"] ?? card?["url"] ?? card?["homepage"])),
            ("Version", cardString(card?["version"])),
            ("Created", cardString(card?["created"] ?? card?["date"] ?? card?["issued"])),
            ("Keywords", cardString(card?["keywords"] ?? card?["tags"])),
        ]
        let present = facts.compactMap { (k, v) in v.map { (k, $0) } }
        if !present.isEmpty {
            out.append(heading("About"))
            for (k, v) in present { out.append(fact(k, v)) }
            out.append(line("", size: 6))
        }

        out.append(heading("Sections"))
        for section in header.sections.sorted(by: { $0.offset < $1.offset }) where section.length > 0 {
            let share = size > 0 ? Double(section.length) / Double(size) * 100 : 0
            out.append(fact(section.name, "\(humanBytes(section.length))  ·  \(String(format: "%.1f", share))%"))
        }
        out.append(line("", size: 6))

        out.append(heading("File"))
        out.append(fact("Format", String(format: "0x%02x", header.version)))
        out.append(fact("Content hash", header.contentHash))
        if card == nil {
            out.append(line("", size: 6))
            out.append(line("This file carries no Dataset Card — the metadata section is empty.",
                            size: 11, color: .secondaryLabelColor, spacingAfter: 0))
        }
        return out
    }

    private func line(_ s: String, size: CGFloat, weight: NSFont.Weight = .regular,
                      color: NSColor = .labelColor, mono: Bool = false,
                      spacingAfter: CGFloat = 6) -> NSAttributedString {
        let para = NSMutableParagraphStyle()
        para.paragraphSpacing = spacingAfter
        para.lineSpacing = 1.5
        let font = mono
            ? NSFont.monospacedSystemFont(ofSize: size, weight: weight)
            : NSFont.systemFont(ofSize: size, weight: weight)
        return NSAttributedString(string: s + "\n", attributes: [
            .font: font, .foregroundColor: color, .paragraphStyle: para,
        ])
    }

    private func heading(_ s: String) -> NSAttributedString {
        let para = NSMutableParagraphStyle()
        para.paragraphSpacing = 5
        return NSAttributedString(string: s.uppercased() + "\n", attributes: [
            .font: NSFont.systemFont(ofSize: 10, weight: .semibold),
            .foregroundColor: NSColor.tertiaryLabelColor,
            .kern: 1.2,
            .paragraphStyle: para,
        ])
    }

    private func fact(_ key: String, _ value: String) -> NSAttributedString {
        let para = NSMutableParagraphStyle()
        para.paragraphSpacing = 3
        para.headIndent = 132
        para.tabStops = [NSTextTab(textAlignment: .left, location: 132)]
        let out = NSMutableAttributedString(string: key + "\t", attributes: [
            .font: NSFont.systemFont(ofSize: 11.5),
            .foregroundColor: NSColor.secondaryLabelColor,
            .paragraphStyle: para,
        ])
        out.append(NSAttributedString(string: value + "\n", attributes: [
            .font: NSFont.systemFont(ofSize: 11.5),
            .foregroundColor: NSColor.labelColor,
            .paragraphStyle: para,
        ]))
        return out
    }
}
