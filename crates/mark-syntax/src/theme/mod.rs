//! Mark theme catalog and product adapter over Syntaxmate's TextMate selector.

use std::sync::OnceLock;

use crate::{HighlightScopeTable, ScopeStackRef, SyntaxRuleOverride};

pub use syntaxmate::{
    FontModifiers as SyntaxModifiers, ResolvedThemeStyle, RgbColor, Style as ResolvedSyntaxStyle,
    ThemeMatch, ThemeSelectorScore,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinTextMateTheme {
    CatppuccinLatte,
    CatppuccinFrappe,
    CatppuccinMacchiato,
    CatppuccinMocha,
    GruvboxDark,
    GruvboxLight,
    GithubDark,
    GithubDarkHighContrast,
    GithubLight,
    GithubLightHighContrast,
    Tokyonight,
    Nordic,
    Nord,
    AyuDark,
    AyuLight,
    AyuMirage,
    Molokai,
    ZenbonesDark,
    ZenbonesLight,
    Duckbones,
    ForestbonesDark,
    ForestbonesLight,
    Kanagawabones,
    NeobonesDark,
    NeobonesLight,
    Nordbones,
    RosebonesDark,
    RosebonesLight,
    SeoulbonesDark,
    SeoulbonesLight,
    TokyobonesDark,
    TokyobonesLight,
    Vimbones,
    Zenburned,
    ZenwrittenDark,
    ZenwrittenLight,
    KanagawaWave,
    KanagawaDragon,
    KanagawaLotus,
    EverforestDark,
    EverforestLight,
    TokenDark,
    TokenLight,
    GruvboxMaterialDark,
    GruvboxMaterialLight,
    Mfd,
    MfdDark,
    MfdStealth,
    MfdAmber,
    MfdMono,
    MfdScarlet,
    MfdPaper,
    MfdHud,
    MfdNvg,
    MfdBlackout,
    MfdFlir,
    MfdFlirBh,
    MfdFlirRh,
    MfdFlirFusion,
    MfdGblLight,
    MfdGblDark,
    MfdLumon,
    MfdNerv,
}

impl BuiltinTextMateTheme {
    pub fn from_name(name: &str) -> Option<Self> {
        Self::all()
            .iter()
            .copied()
            .find(|theme| theme.name() == name)
            .or(match name {
                "tokyo-night" => Some(Self::Tokyonight),
                "ayu" => Some(Self::AyuDark),
                "monokai" => Some(Self::Molokai),
                "zenbones" => Some(Self::ZenbonesDark),
                "forestbones" => Some(Self::ForestbonesDark),
                "neobones" => Some(Self::NeobonesDark),
                "rosebones" => Some(Self::RosebonesDark),
                "seoulbones" => Some(Self::SeoulbonesDark),
                "tokyobones" => Some(Self::TokyobonesDark),
                "zenwritten" => Some(Self::ZenwrittenDark),
                "kanagawa" => Some(Self::KanagawaWave),
                "everforest" => Some(Self::EverforestDark),
                "token" => Some(Self::TokenDark),
                "gruvbox-material" => Some(Self::GruvboxMaterialDark),
                _ => None,
            })
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::CatppuccinLatte => "catppuccin-latte",
            Self::CatppuccinFrappe => "catppuccin-frappe",
            Self::CatppuccinMacchiato => "catppuccin-macchiato",
            Self::CatppuccinMocha => "catppuccin-mocha",
            Self::GruvboxDark => "gruvbox-dark",
            Self::GruvboxLight => "gruvbox-light",
            Self::GithubDark => "github-dark",
            Self::GithubDarkHighContrast => "github-dark-high-contrast",
            Self::GithubLight => "github-light",
            Self::GithubLightHighContrast => "github-light-high-contrast",
            Self::Tokyonight => "tokyonight",
            Self::Nordic => "nordic",
            Self::Nord => "nord",
            Self::AyuDark => "ayu-dark",
            Self::AyuLight => "ayu-light",
            Self::AyuMirage => "ayu-mirage",
            Self::Molokai => "molokai",
            Self::ZenbonesDark => "zenbones-dark",
            Self::ZenbonesLight => "zenbones-light",
            Self::Duckbones => "duckbones",
            Self::ForestbonesDark => "forestbones-dark",
            Self::ForestbonesLight => "forestbones-light",
            Self::Kanagawabones => "kanagawabones",
            Self::NeobonesDark => "neobones-dark",
            Self::NeobonesLight => "neobones-light",
            Self::Nordbones => "nordbones",
            Self::RosebonesDark => "rosebones-dark",
            Self::RosebonesLight => "rosebones-light",
            Self::SeoulbonesDark => "seoulbones-dark",
            Self::SeoulbonesLight => "seoulbones-light",
            Self::TokyobonesDark => "tokyobones-dark",
            Self::TokyobonesLight => "tokyobones-light",
            Self::Vimbones => "vimbones",
            Self::Zenburned => "zenburned",
            Self::ZenwrittenDark => "zenwritten-dark",
            Self::ZenwrittenLight => "zenwritten-light",
            Self::KanagawaWave => "kanagawa-wave",
            Self::KanagawaDragon => "kanagawa-dragon",
            Self::KanagawaLotus => "kanagawa-lotus",
            Self::EverforestDark => "everforest-dark",
            Self::EverforestLight => "everforest-light",
            Self::TokenDark => "token-dark",
            Self::TokenLight => "token-light",
            Self::GruvboxMaterialDark => "gruvbox-material-dark",
            Self::GruvboxMaterialLight => "gruvbox-material-light",
            Self::Mfd => "mfd",
            Self::MfdDark => "mfd-dark",
            Self::MfdStealth => "mfd-stealth",
            Self::MfdAmber => "mfd-amber",
            Self::MfdMono => "mfd-mono",
            Self::MfdScarlet => "mfd-scarlet",
            Self::MfdPaper => "mfd-paper",
            Self::MfdHud => "mfd-hud",
            Self::MfdNvg => "mfd-nvg",
            Self::MfdBlackout => "mfd-blackout",
            Self::MfdFlir => "mfd-flir",
            Self::MfdFlirBh => "mfd-flir-bh",
            Self::MfdFlirRh => "mfd-flir-rh",
            Self::MfdFlirFusion => "mfd-flir-fusion",
            Self::MfdGblLight => "mfd-gbl-light",
            Self::MfdGblDark => "mfd-gbl-dark",
            Self::MfdLumon => "mfd-lumon",
            Self::MfdNerv => "mfd-nerv",
        }
    }

    pub fn get(self) -> &'static TextMateTheme {
        builtin_theme(self)
    }

    pub const fn all() -> &'static [Self] {
        &[
            Self::CatppuccinLatte,
            Self::CatppuccinFrappe,
            Self::CatppuccinMacchiato,
            Self::CatppuccinMocha,
            Self::GruvboxDark,
            Self::GruvboxLight,
            Self::GithubDark,
            Self::GithubDarkHighContrast,
            Self::GithubLight,
            Self::GithubLightHighContrast,
            Self::Tokyonight,
            Self::Nordic,
            Self::Nord,
            Self::AyuDark,
            Self::AyuLight,
            Self::AyuMirage,
            Self::Molokai,
            Self::ZenbonesDark,
            Self::ZenbonesLight,
            Self::Duckbones,
            Self::ForestbonesDark,
            Self::ForestbonesLight,
            Self::Kanagawabones,
            Self::NeobonesDark,
            Self::NeobonesLight,
            Self::Nordbones,
            Self::RosebonesDark,
            Self::RosebonesLight,
            Self::SeoulbonesDark,
            Self::SeoulbonesLight,
            Self::TokyobonesDark,
            Self::TokyobonesLight,
            Self::Vimbones,
            Self::Zenburned,
            Self::ZenwrittenDark,
            Self::ZenwrittenLight,
            Self::KanagawaWave,
            Self::KanagawaDragon,
            Self::KanagawaLotus,
            Self::EverforestDark,
            Self::EverforestLight,
            Self::TokenDark,
            Self::TokenLight,
            Self::GruvboxMaterialDark,
            Self::GruvboxMaterialLight,
            Self::Mfd,
            Self::MfdDark,
            Self::MfdStealth,
            Self::MfdAmber,
            Self::MfdMono,
            Self::MfdScarlet,
            Self::MfdPaper,
            Self::MfdHud,
            Self::MfdNvg,
            Self::MfdBlackout,
            Self::MfdFlir,
            Self::MfdFlirBh,
            Self::MfdFlirRh,
            Self::MfdFlirFusion,
            Self::MfdGblLight,
            Self::MfdGblDark,
            Self::MfdLumon,
            Self::MfdNerv,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextMateTheme {
    inner: syntaxmate::TextMateTheme,
}

impl TextMateTheme {
    pub fn from_json(json: &str) -> Result<Self, String> {
        syntaxmate::TextMateTheme::from_json(json).map(|inner| Self { inner })
    }

    pub fn from_syntax_rules(rules: &[SyntaxRuleOverride]) -> Result<Self, String> {
        let rules = rules
            .iter()
            .map(|rule| syntaxmate::ThemeRule {
                scope: rule.scope.clone(),
                foreground: rule.foreground.clone(),
                background: rule.background.clone(),
                font_style: rule.font_style.clone(),
            })
            .collect::<Vec<_>>();
        syntaxmate::TextMateTheme::from_rules(&rules).map(|inner| Self { inner })
    }

    pub fn name(&self) -> &str {
        self.inner.name()
    }

    pub fn default_style(&self) -> ResolvedSyntaxStyle {
        self.inner.default_style()
    }

    pub fn color(&self, name: &str) -> Option<RgbColor> {
        self.inner.color(name)
    }

    pub fn resolve(
        &self,
        table: &HighlightScopeTable,
        stack: ScopeStackRef,
    ) -> ResolvedSyntaxStyle {
        self.inner.resolve(table, stack)
    }

    pub fn resolve_style(
        &self,
        table: &HighlightScopeTable,
        stack: ScopeStackRef,
    ) -> ResolvedThemeStyle {
        self.inner.resolve_style(table, stack)
    }

    pub fn resolve_with_match<'a>(
        &'a self,
        table: &HighlightScopeTable,
        stack: ScopeStackRef,
    ) -> ThemeMatch<'a> {
        self.inner.resolve_with_match(table, stack)
    }
}

macro_rules! vendored_theme {
    ($function:ident, $file:literal) => {
        pub fn $function() -> &'static TextMateTheme {
            static THEME: OnceLock<TextMateTheme> = OnceLock::new();
            THEME.get_or_init(|| {
                TextMateTheme::from_json(include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../assets/themes/",
                    $file
                )))
                .unwrap_or_else(|error| panic!("vendored theme {} is invalid: {error}", $file))
            })
        }
    };
}

vendored_theme!(catppuccin_latte, "catppuccin-latte.json");
vendored_theme!(catppuccin_frappe, "catppuccin-frappe.json");
vendored_theme!(catppuccin_macchiato, "catppuccin-macchiato.json");
vendored_theme!(catppuccin_mocha, "catppuccin-mocha.json");
vendored_theme!(gruvbox_dark, "gruvbox-dark.json");
vendored_theme!(gruvbox_light, "gruvbox-light.json");
vendored_theme!(github_dark, "github-dark.json");
vendored_theme!(github_dark_high_contrast, "github-dark-high-contrast.json");
vendored_theme!(github_light, "github-light.json");
vendored_theme!(
    github_light_high_contrast,
    "github-light-high-contrast.json"
);
vendored_theme!(tokyonight, "tokyonight.json");
vendored_theme!(nordic, "nordic.json");
vendored_theme!(nord, "nord.json");
vendored_theme!(ayu_dark, "ayu-dark.json");
vendored_theme!(ayu_light, "ayu-light.json");
vendored_theme!(ayu_mirage, "ayu-mirage.json");
vendored_theme!(molokai, "molokai.json");
vendored_theme!(zenbones_dark, "zenbones-dark.json");
vendored_theme!(zenbones_light, "zenbones-light.json");
vendored_theme!(duckbones, "duckbones.json");
vendored_theme!(forestbones_dark, "forestbones-dark.json");
vendored_theme!(forestbones_light, "forestbones-light.json");
vendored_theme!(kanagawabones, "kanagawabones.json");
vendored_theme!(neobones_dark, "neobones-dark.json");
vendored_theme!(neobones_light, "neobones-light.json");
vendored_theme!(nordbones, "nordbones.json");
vendored_theme!(rosebones_dark, "rosebones-dark.json");
vendored_theme!(rosebones_light, "rosebones-light.json");
vendored_theme!(seoulbones_dark, "seoulbones-dark.json");
vendored_theme!(seoulbones_light, "seoulbones-light.json");
vendored_theme!(tokyobones_dark, "tokyobones-dark.json");
vendored_theme!(tokyobones_light, "tokyobones-light.json");
vendored_theme!(vimbones, "vimbones.json");
vendored_theme!(zenburned, "zenburned.json");
vendored_theme!(zenwritten_dark, "zenwritten-dark.json");
vendored_theme!(zenwritten_light, "zenwritten-light.json");
vendored_theme!(kanagawa_wave, "kanagawa-wave.json");
vendored_theme!(kanagawa_dragon, "kanagawa-dragon.json");
vendored_theme!(kanagawa_lotus, "kanagawa-lotus.json");
vendored_theme!(everforest_dark, "everforest-dark.json");
vendored_theme!(everforest_light, "everforest-light.json");
vendored_theme!(token_dark, "token-dark.json");
vendored_theme!(token_light, "token-light.json");
vendored_theme!(gruvbox_material_dark, "gruvbox-material-dark.json");
vendored_theme!(gruvbox_material_light, "gruvbox-material-light.json");
vendored_theme!(mfd, "mfd.json");
vendored_theme!(mfd_dark, "mfd-dark.json");
vendored_theme!(mfd_stealth, "mfd-stealth.json");
vendored_theme!(mfd_amber, "mfd-amber.json");
vendored_theme!(mfd_mono, "mfd-mono.json");
vendored_theme!(mfd_scarlet, "mfd-scarlet.json");
vendored_theme!(mfd_paper, "mfd-paper.json");
vendored_theme!(mfd_hud, "mfd-hud.json");
vendored_theme!(mfd_nvg, "mfd-nvg.json");
vendored_theme!(mfd_blackout, "mfd-blackout.json");
vendored_theme!(mfd_flir, "mfd-flir.json");
vendored_theme!(mfd_flir_bh, "mfd-flir-bh.json");
vendored_theme!(mfd_flir_rh, "mfd-flir-rh.json");
vendored_theme!(mfd_flir_fusion, "mfd-flir-fusion.json");
vendored_theme!(mfd_gbl_light, "mfd-gbl-light.json");
vendored_theme!(mfd_gbl_dark, "mfd-gbl-dark.json");
vendored_theme!(mfd_lumon, "mfd-lumon.json");
vendored_theme!(mfd_nerv, "mfd-nerv.json");

fn builtin_theme(theme: BuiltinTextMateTheme) -> &'static TextMateTheme {
    match theme {
        BuiltinTextMateTheme::CatppuccinLatte => catppuccin_latte(),
        BuiltinTextMateTheme::CatppuccinFrappe => catppuccin_frappe(),
        BuiltinTextMateTheme::CatppuccinMacchiato => catppuccin_macchiato(),
        BuiltinTextMateTheme::CatppuccinMocha => catppuccin_mocha(),
        BuiltinTextMateTheme::GruvboxDark => gruvbox_dark(),
        BuiltinTextMateTheme::GruvboxLight => gruvbox_light(),
        BuiltinTextMateTheme::GithubDark => github_dark(),
        BuiltinTextMateTheme::GithubDarkHighContrast => github_dark_high_contrast(),
        BuiltinTextMateTheme::GithubLight => github_light(),
        BuiltinTextMateTheme::GithubLightHighContrast => github_light_high_contrast(),
        BuiltinTextMateTheme::Tokyonight => tokyonight(),
        BuiltinTextMateTheme::Nordic => nordic(),
        BuiltinTextMateTheme::Nord => nord(),
        BuiltinTextMateTheme::AyuDark => ayu_dark(),
        BuiltinTextMateTheme::AyuLight => ayu_light(),
        BuiltinTextMateTheme::AyuMirage => ayu_mirage(),
        BuiltinTextMateTheme::Molokai => molokai(),
        BuiltinTextMateTheme::ZenbonesDark => zenbones_dark(),
        BuiltinTextMateTheme::ZenbonesLight => zenbones_light(),
        BuiltinTextMateTheme::Duckbones => duckbones(),
        BuiltinTextMateTheme::ForestbonesDark => forestbones_dark(),
        BuiltinTextMateTheme::ForestbonesLight => forestbones_light(),
        BuiltinTextMateTheme::Kanagawabones => kanagawabones(),
        BuiltinTextMateTheme::NeobonesDark => neobones_dark(),
        BuiltinTextMateTheme::NeobonesLight => neobones_light(),
        BuiltinTextMateTheme::Nordbones => nordbones(),
        BuiltinTextMateTheme::RosebonesDark => rosebones_dark(),
        BuiltinTextMateTheme::RosebonesLight => rosebones_light(),
        BuiltinTextMateTheme::SeoulbonesDark => seoulbones_dark(),
        BuiltinTextMateTheme::SeoulbonesLight => seoulbones_light(),
        BuiltinTextMateTheme::TokyobonesDark => tokyobones_dark(),
        BuiltinTextMateTheme::TokyobonesLight => tokyobones_light(),
        BuiltinTextMateTheme::Vimbones => vimbones(),
        BuiltinTextMateTheme::Zenburned => zenburned(),
        BuiltinTextMateTheme::ZenwrittenDark => zenwritten_dark(),
        BuiltinTextMateTheme::ZenwrittenLight => zenwritten_light(),
        BuiltinTextMateTheme::KanagawaWave => kanagawa_wave(),
        BuiltinTextMateTheme::KanagawaDragon => kanagawa_dragon(),
        BuiltinTextMateTheme::KanagawaLotus => kanagawa_lotus(),
        BuiltinTextMateTheme::EverforestDark => everforest_dark(),
        BuiltinTextMateTheme::EverforestLight => everforest_light(),
        BuiltinTextMateTheme::TokenDark => token_dark(),
        BuiltinTextMateTheme::TokenLight => token_light(),
        BuiltinTextMateTheme::GruvboxMaterialDark => gruvbox_material_dark(),
        BuiltinTextMateTheme::GruvboxMaterialLight => gruvbox_material_light(),
        BuiltinTextMateTheme::Mfd => mfd(),
        BuiltinTextMateTheme::MfdDark => mfd_dark(),
        BuiltinTextMateTheme::MfdStealth => mfd_stealth(),
        BuiltinTextMateTheme::MfdAmber => mfd_amber(),
        BuiltinTextMateTheme::MfdMono => mfd_mono(),
        BuiltinTextMateTheme::MfdScarlet => mfd_scarlet(),
        BuiltinTextMateTheme::MfdPaper => mfd_paper(),
        BuiltinTextMateTheme::MfdHud => mfd_hud(),
        BuiltinTextMateTheme::MfdNvg => mfd_nvg(),
        BuiltinTextMateTheme::MfdBlackout => mfd_blackout(),
        BuiltinTextMateTheme::MfdFlir => mfd_flir(),
        BuiltinTextMateTheme::MfdFlirBh => mfd_flir_bh(),
        BuiltinTextMateTheme::MfdFlirRh => mfd_flir_rh(),
        BuiltinTextMateTheme::MfdFlirFusion => mfd_flir_fusion(),
        BuiltinTextMateTheme::MfdGblLight => mfd_gbl_light(),
        BuiltinTextMateTheme::MfdGblDark => mfd_gbl_dark(),
        BuiltinTextMateTheme::MfdLumon => mfd_lumon(),
        BuiltinTextMateTheme::MfdNerv => mfd_nerv(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_named_builtin_theme_loads() {
        for theme in BuiltinTextMateTheme::all() {
            assert!(!theme.get().name().is_empty(), "{}", theme.name());
        }
    }

    #[test]
    fn exact_scope_resolution_is_delegated_to_syntaxmate() {
        let theme = github_dark_high_contrast();
        let (table, stack) = HighlightScopeTable::from_scope_names(&[
            "source.rust",
            "meta.function.definition.rust",
            "entity.name.function.rust",
        ]);
        let matched = theme.resolve_with_match(&table, stack);
        assert!(matched.foreground_matched);
        assert!(matched.selector.is_some());
    }

    #[test]
    fn user_syntax_rules_use_syntaxmate_selector_matching() {
        let theme = TextMateTheme::from_syntax_rules(&[SyntaxRuleOverride {
            scope: "meta.function entity.name.function".to_owned(),
            foreground: Some("#123456".to_owned()),
            background: None,
            font_style: Some("bold".to_owned()),
        }])
        .unwrap();
        let (table, stack) = HighlightScopeTable::from_scope_names(&[
            "source.test",
            "meta.function.test",
            "entity.name.function.test",
        ]);
        let style = theme.resolve(&table, stack);
        assert_eq!(
            style.foreground,
            Some(RgbColor {
                red: 0x12,
                green: 0x34,
                blue: 0x56
            })
        );
        assert!(style.modifiers.contains(SyntaxModifiers::BOLD));
    }
}
