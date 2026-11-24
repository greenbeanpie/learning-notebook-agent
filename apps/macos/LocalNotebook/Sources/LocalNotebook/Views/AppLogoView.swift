import SwiftUI

/// Crisp vector rendering of the cross-platform Local Notebook Agent logo (`assets/logo.svg`).
struct AppLogoView: View {
    var size: CGFloat = 30
    var accent: Color = Color(red: 0xA8/255, green: 0xC7/255, blue: 0xFA/255)

    var body: some View {
        Canvas { ctx, canvasSize in
            let s = min(canvasSize.width, canvasSize.height) / 512.0
            ctx.scaleBy(x: s, y: s)

            // Outer squircle badge
            let badgeRect = CGRect(x: 24, y: 24, width: 464, height: 464)
            let badgePath = Path(roundedRect: badgeRect, cornerRadius: 112)
            ctx.fill(
                badgePath,
                with: .linearGradient(
                    Gradient(colors: [
                        Color(red: 0x25/255, green: 0x28/255, blue: 0x30/255),
                        Color(red: 0x19/255, green: 0x1B/255, blue: 0x20/255),
                        Color(red: 0x11/255, green: 0x12/255, blue: 0x16/255),
                    ]),
                    startPoint: CGPoint(x: 60, y: 40),
                    endPoint: CGPoint(x: 452, y: 472)
                )
            )

            // Stacked back page
            let backPage = Path(roundedRect: CGRect(x: 148, y: 102, width: 234, height: 296), cornerRadius: 34)
            ctx.fill(backPage, with: .color(Color(red: 0x23/255, green: 0x26/255, blue: 0x2E/255).opacity(0.72)))
            ctx.stroke(backPage, with: .color(Color(red: 0x3C/255, green: 0x40/255, blue: 0x48/255).opacity(0.72)), lineWidth: 4)

            // Main notebook cover
            let coverRect = CGRect(x: 128, y: 114, width: 234, height: 296)
            let coverPath = Path(roundedRect: coverRect, cornerRadius: 34)
            ctx.fill(
                coverPath,
                with: .linearGradient(
                    Gradient(colors: [
                        Color(red: 0x2E/255, green: 0x32/255, blue: 0x3C/255),
                        Color(red: 0x1E/255, green: 0x20/255, blue: 0x27/255),
                    ]),
                    startPoint: CGPoint(x: 128, y: 114),
                    endPoint: CGPoint(x: 362, y: 410)
                )
            )

            // Spine tint clipped to cover
            ctx.drawLayer { layerCtx in
                layerCtx.clip(to: coverPath)
                let spineRect = CGRect(x: 128, y: 114, width: 54, height: 296)
                layerCtx.fill(Path(spineRect), with: .color(accent.opacity(0.22)))
                var spineLine = Path()
                spineLine.move(to: CGPoint(x: 182, y: 114))
                spineLine.addLine(to: CGPoint(x: 182, y: 410))
                layerCtx.stroke(spineLine, with: .color(accent.opacity(0.65)), lineWidth: 6)

                // Bookmark ribbon
                var bookmark = Path()
                bookmark.move(to: CGPoint(x: 294, y: 114))
                bookmark.addLine(to: CGPoint(x: 326, y: 114))
                bookmark.addLine(to: CGPoint(x: 326, y: 174))
                bookmark.addLine(to: CGPoint(x: 310, y: 160))
                bookmark.addLine(to: CGPoint(x: 294, y: 174))
                bookmark.closeSubpath()
                layerCtx.fill(
                    bookmark,
                    with: .linearGradient(
                        Gradient(colors: [
                            Color(red: 0xFD/255, green: 0xE0/255, blue: 0x8D/255),
                            Color(red: 0xF5/255, green: 0xB9/255, blue: 0x42/255),
                        ]),
                        startPoint: CGPoint(x: 310, y: 114),
                        endPoint: CGPoint(x: 310, y: 174)
                    )
                )
            }

            // Cover outline
            ctx.stroke(
                coverPath,
                with: .linearGradient(
                    Gradient(colors: [accent.opacity(0.95), accent]),
                    startPoint: CGPoint(x: 128, y: 114),
                    endPoint: CGPoint(x: 362, y: 410)
                ),
                lineWidth: 11
            )

            // Content lines
            let lines: [(CGFloat, CGFloat, Color, CGFloat)] = [
                (202, 322, Color(red: 0xEE/255, green: 0xF2/255, blue: 0xF8/255), 13),
                (244, 306, Color(red: 0xBF/255, green: 0xC7/255, blue: 0xD4/255), 12),
                (286, 274, Color(red: 0x8C/255, green: 0x95/255, blue: 0xA6/255), 12),
            ]
            for (y, endX, color, width) in lines {
                var p = Path()
                p.move(to: CGPoint(x: 212, y: y))
                p.addLine(to: CGPoint(x: endX, y: y))
                ctx.stroke(p, with: .color(color), style: StrokeStyle(lineWidth: width, lineCap: .round))
            }

            // Ambient glow behind sparkle
            let glowCircle = Path(ellipseIn: CGRect(x: 260, y: 262, width: 176, height: 176))
            ctx.fill(
                glowCircle,
                with: .radialGradient(
                    Gradient(colors: [accent.opacity(0.45), accent.opacity(0.12), accent.opacity(0)]),
                    center: CGPoint(x: 348, y: 350),
                    startRadius: 4,
                    endRadius: 88
                )
            )

            // Primary 4-point AI sparkle
            var sparkle = Path()
            sparkle.move(to: CGPoint(x: 348, y: 286))
            sparkle.addCurve(to: CGPoint(x: 410, y: 348), control1: CGPoint(x: 352, y: 322), control2: CGPoint(x: 374, y: 344))
            sparkle.addCurve(to: CGPoint(x: 348, y: 410), control1: CGPoint(x: 374, y: 352), control2: CGPoint(x: 352, y: 374))
            sparkle.addCurve(to: CGPoint(x: 286, y: 348), control1: CGPoint(x: 344, y: 374), control2: CGPoint(x: 322, y: 352))
            sparkle.addCurve(to: CGPoint(x: 348, y: 286), control1: CGPoint(x: 322, y: 344), control2: CGPoint(x: 344, y: 322))
            sparkle.closeSubpath()
            ctx.fill(sparkle, with: .color(accent))

            // Secondary micro-sparkle
            var micro = Path()
            micro.move(to: CGPoint(x: 396, y: 270))
            micro.addCurve(to: CGPoint(x: 420, y: 294), control1: CGPoint(x: 398, y: 284), control2: CGPoint(x: 406, y: 292))
            micro.addCurve(to: CGPoint(x: 396, y: 318), control1: CGPoint(x: 406, y: 296), control2: CGPoint(x: 398, y: 304))
            micro.addCurve(to: CGPoint(x: 372, y: 294), control1: CGPoint(x: 394, y: 304), control2: CGPoint(x: 386, y: 296))
            micro.addCurve(to: CGPoint(x: 396, y: 270), control1: CGPoint(x: 386, y: 292), control2: CGPoint(x: 394, y: 284))
            micro.closeSubpath()
            ctx.fill(micro, with: .color(Color.white.opacity(0.9)))
        }
        .frame(width: size, height: size)
        .accessibilityLabel("Local Notebook Agent Logo")
    }
}
