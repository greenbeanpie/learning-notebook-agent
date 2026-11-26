import SwiftUI

enum AppThemeId: String, CaseIterable, Identifiable {
    case dark = "dark"
    case light = "light"
    case system = "system"
    case midnight = "midnight"
    case forest = "forest"
    case warmPaper = "warm_paper"
    case roseDusk = "rose_dusk"

    var id: String { rawValue }

    static func resolve(_ raw: String) -> AppThemeId {
        AppThemeId(rawValue: raw) ?? .dark
    }

    func displayName(en: Bool) -> String {
        switch self {
        case .dark: return L10n.t("settings.theme_dark", en: en)
        case .light: return L10n.t("settings.theme_light", en: en)
        case .system: return L10n.t("settings.theme_system", en: en)
        case .midnight: return L10n.t("settings.theme_midnight", en: en)
        case .forest: return L10n.t("settings.theme_forest", en: en)
        case .warmPaper: return L10n.t("settings.theme_warm_paper", en: en)
        case .roseDusk: return L10n.t("settings.theme_rose_dusk", en: en)
        }
    }

    func subtitle(en: Bool) -> String {
        switch self {
        case .dark: return L10n.t("theme.classic_neutral_dark", en: en)
        case .light: return L10n.t("theme.crisp_daylight_surfaces", en: en)
        case .system: return L10n.t("theme.matches_macos_appearance", en: en)
        case .midnight: return L10n.t("theme.deep_navy_indigo_glow", en: en)
        case .forest: return L10n.t("theme.calm_pine_mint_accent", en: en)
        case .warmPaper: return L10n.t("theme.soft_cream_study_paper", en: en)
        case .roseDusk: return L10n.t("theme.plum_charcoal_rose", en: en)
        }
    }

    var preferredColorScheme: ColorScheme? {
        switch self {
        case .system: return nil
        case .light, .warmPaper: return .light
        case .dark, .midnight, .forest, .roseDusk: return .dark
        }
    }

    func palette(systemScheme: ColorScheme = .dark) -> AppThemePalette {
        switch self {
        case .dark:
            return .obsidianDark
        case .light:
            return .paperLight
        case .system:
            return systemScheme == .light ? .paperLight : .obsidianDark
        case .midnight:
            return .midnightIndigo
        case .forest:
            return .nordicForest
        case .warmPaper:
            return .warmParchment
        case .roseDusk:
            return .roseDusk
        }
    }
}

struct AppThemePalette {
    let isDark: Bool
    let canvasBg: Color
    let panelBg: Color
    let elevatedBg: Color
    let hoverBg: Color
    let subtleBorder: Color
    let strongBorder: Color
    let textPrimary: Color
    let textSecondary: Color
    let textMuted: Color
    let accent: Color
    let accentText: Color
    let kgBannerBg: Color
    let kgBannerBorder: Color
    let kgBannerText: Color

    /// Default appearance — matches Windows `App.xaml` exactly.
    static let obsidianDark = AppThemePalette(
        isDark: true,
        canvasBg: Color(hex: 0x131314),
        panelBg: Color(hex: 0x1E1F20),
        elevatedBg: Color(hex: 0x282A2C),
        hoverBg: Color(hex: 0x333537),
        subtleBorder: Color.white.opacity(0.08),
        strongBorder: Color(hex: 0x3C4043),
        textPrimary: Color(hex: 0xE3E3E3),
        textSecondary: Color(hex: 0xC4C7C5),
        textMuted: Color(hex: 0x8E918F),
        accent: Color(hex: 0xA8C7FA),
        accentText: Color(hex: 0x131314),
        kgBannerBg: Color(hex: 0x223444),
        kgBannerBorder: Color(hex: 0x5286BA),
        kgBannerText: Color(hex: 0xD4E4F8)
    )

    static let paperLight = AppThemePalette(
        isDark: false,
        canvasBg: Color(hex: 0xF4F6F9),
        panelBg: Color(hex: 0xFFFFFF),
        elevatedBg: Color(hex: 0xF0F3F8),
        hoverBg: Color(hex: 0xE5EAF2),
        subtleBorder: Color.black.opacity(0.08),
        strongBorder: Color(hex: 0xCBD5E1),
        textPrimary: Color(hex: 0x1F2328),
        textSecondary: Color(hex: 0x4B5563),
        textMuted: Color(hex: 0x6B7280),
        accent: Color(hex: 0x1A73E8),
        accentText: Color.white,
        kgBannerBg: Color(hex: 0xE8F0FE),
        kgBannerBorder: Color(hex: 0x8AB4F8),
        kgBannerText: Color(hex: 0x174EA6)
    )

    static let midnightIndigo = AppThemePalette(
        isDark: true,
        canvasBg: Color(hex: 0x0D1117),
        panelBg: Color(hex: 0x161B26),
        elevatedBg: Color(hex: 0x1F2737),
        hoverBg: Color(hex: 0x293347),
        subtleBorder: Color(hex: 0x818CF8).opacity(0.14),
        strongBorder: Color(hex: 0x38455E),
        textPrimary: Color(hex: 0xE6EDF5),
        textSecondary: Color(hex: 0xB8C4D8),
        textMuted: Color(hex: 0x7D8DA6),
        accent: Color(hex: 0x818CF8),
        accentText: Color(hex: 0x0D1117),
        kgBannerBg: Color(hex: 0x1E2542),
        kgBannerBorder: Color(hex: 0x6366F1),
        kgBannerText: Color(hex: 0xDCE1FF)
    )

    static let nordicForest = AppThemePalette(
        isDark: true,
        canvasBg: Color(hex: 0x0F1614),
        panelBg: Color(hex: 0x17221F),
        elevatedBg: Color(hex: 0x202E2A),
        hoverBg: Color(hex: 0x2A3C37),
        subtleBorder: Color(hex: 0x6EE7B7).opacity(0.12),
        strongBorder: Color(hex: 0x354F47),
        textPrimary: Color(hex: 0xE4EFEA),
        textSecondary: Color(hex: 0xB6CCC3),
        textMuted: Color(hex: 0x7F9A90),
        accent: Color(hex: 0x6EE7B7),
        accentText: Color(hex: 0x0F1614),
        kgBannerBg: Color(hex: 0x19332B),
        kgBannerBorder: Color(hex: 0x34D399),
        kgBannerText: Color(hex: 0xD1FAE5)
    )

    static let warmParchment = AppThemePalette(
        isDark: false,
        canvasBg: Color(hex: 0xF5EFE6),
        panelBg: Color(hex: 0xFFFDF8),
        elevatedBg: Color(hex: 0xEFE7DA),
        hoverBg: Color(hex: 0xE5DAC9),
        subtleBorder: Color(hex: 0x5C4630).opacity(0.12),
        strongBorder: Color(hex: 0xCBBBA4),
        textPrimary: Color(hex: 0x2C2218),
        textSecondary: Color(hex: 0x5B4B3A),
        textMuted: Color(hex: 0x83715D),
        accent: Color(hex: 0xB45309),
        accentText: Color.white,
        kgBannerBg: Color(hex: 0xFEF3C7),
        kgBannerBorder: Color(hex: 0xD97706),
        kgBannerText: Color(hex: 0x78350F)
    )

    static let roseDusk = AppThemePalette(
        isDark: true,
        canvasBg: Color(hex: 0x161217),
        panelBg: Color(hex: 0x211B23),
        elevatedBg: Color(hex: 0x2C242F),
        hoverBg: Color(hex: 0x392F3D),
        subtleBorder: Color(hex: 0xF4A5C4).opacity(0.12),
        strongBorder: Color(hex: 0x4E3F54),
        textPrimary: Color(hex: 0xF1E8F2),
        textSecondary: Color(hex: 0xCABCC9),
        textMuted: Color(hex: 0x958496),
        accent: Color(hex: 0xF4A5C4),
        accentText: Color(hex: 0x161217),
        kgBannerBg: Color(hex: 0x372234),
        kgBannerBorder: Color(hex: 0xD97CA3),
        kgBannerText: Color(hex: 0xFCE7F3)
    )
}

extension Color {
    init(hex: UInt32, alpha: Double = 1.0) {
        self.init(
            red: Double((hex >> 16) & 0xFF) / 255.0,
            green: Double((hex >> 8) & 0xFF) / 255.0,
            blue: Double(hex & 0xFF) / 255.0,
            opacity: alpha
        )
    }

    /// Windows-synced Studio tile accent colors.
    static func studioKindColor(_ kind: String) -> Color {
        switch kind {
        case "audio": return Color(hex: 0xA8C7FA)
        case "slides": return Color(hex: 0xFDD663)
        case "mindmap": return Color(hex: 0xC58AF9)
        case "report": return Color(hex: 0x81C995)
        case "flashcards": return Color(hex: 0xF6AEA9)
        case "quiz": return Color(hex: 0x78D9EC)
        case "infographic": return Color(hex: 0xFCAD70)
        case "knowledge_graph": return Color(hex: 0xA8C7FA)
        case "lecture": return Color(hex: 0xF28B82)
        default: return Color(hex: 0xA8C7FA)
        }
    }

    /// Windows-synced Source kind icon colors & SF Symbol names.
    static func sourceKindStyle(kind: String, title: String) -> (symbol: String, color: Color) {
        var k = kind.lowercased()
        let lower = title.lowercased()
        if lower.hasSuffix(".pdf") { k = "pdf" }
        else if lower.hasSuffix(".md") || lower.hasSuffix(".markdown") { k = "md" }
        else if k.isEmpty && lower.hasSuffix(".txt") { k = "txt" }

        switch k {
        case "pdf": return ("doc.richtext.fill", Color(hex: 0xF28B82))
        case "md", "markdown": return ("doc.text.fill", Color(hex: 0x8AB4F8))
        case "docx", "odt", "rtf", "hwp", "hwpx": return ("doc.fill", Color(hex: 0x669DF6))
        case "csv": return ("tablecells.fill", Color(hex: 0x5BB974))
        case "pptx": return ("rectangle.on.rectangle.angled", Color(hex: 0xFCAD70))
        case "epub": return ("book.closed.fill", Color(hex: 0xC58AF9))
        case "image": return ("photo.fill", Color(hex: 0x78D9EC))
        case "audio": return ("waveform", Color(hex: 0xF48FB1))
        case "video": return ("video.fill", Color(hex: 0xEE675C))
        case "txt": return ("doc.plaintext.fill", Color(hex: 0x81C995))
        case "web": return ("globe", Color(hex: 0x78D9EC))
        case "note": return ("note.text", Color(hex: 0xFDD663))
        default: return ("doc.text", Color(hex: 0xC4C7C5))
        }
    }
}
