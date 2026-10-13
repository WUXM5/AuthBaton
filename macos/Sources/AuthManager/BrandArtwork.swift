import AppKit
import SwiftUI

@MainActor
enum BrandArtwork {
    static let mark: NSImage = {
        let url = Bundle.module.url(forResource: "BrandMark", withExtension: "png")!
        return NSImage(contentsOf: url)!
    }()

    static let menuIcon: NSImage = {
        let url = Bundle.module.url(forResource: "MenuMark", withExtension: "png")!
        let image = NSImage(contentsOf: url)!
        image.size = NSSize(width: 18, height: 18)
        image.isTemplate = true
        return image
    }()

    private static var badged: [NSColor: NSImage] = [:]

    // A template image cannot carry colour, so the badged variant tints the mark with the
    // menu bar's own label colour at draw time and adds a coloured dot.
    static func menuIcon(badge: NSColor?) -> NSImage {
        guard let badge else { return menuIcon }
        if let cached = badged[badge] { return cached }
        let base = menuIcon
        let image = NSImage(size: NSSize(width: 18, height: 18), flipped: false) { rect in
            guard let context = NSGraphicsContext.current?.cgContext else { return false }
            context.beginTransparencyLayer(auxiliaryInfo: nil)
            base.draw(in: rect)
            NSColor.labelColor.setFill()
            rect.fill(using: .sourceAtop)
            let dot = NSRect(x: rect.maxX - 7, y: rect.maxY - 7, width: 7, height: 7)
            NSGraphicsContext.current?.compositingOperation = .clear
            NSBezierPath(ovalIn: dot.insetBy(dx: -1.5, dy: -1.5)).fill()
            NSGraphicsContext.current?.compositingOperation = .sourceOver
            badge.setFill()
            NSBezierPath(ovalIn: dot).fill()
            context.endTransparencyLayer()
            return true
        }
        image.isTemplate = false
        badged[badge] = image
        return image
    }

    static let appIcon: NSImage = {
        let url = Bundle.module.url(forResource: "AppIcon", withExtension: "png")!
        return NSImage(contentsOf: url)!
    }()
}

struct BrandMark: View {
    var body: some View {
        Image(nsImage: BrandArtwork.mark)
            .resizable()
            .scaledToFit()
            .accessibilityLabel("AuthManager · 无垠")
    }
}
