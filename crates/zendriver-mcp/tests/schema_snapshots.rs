//! JSON Schema snapshots for every public tool input + output type.
//!
//! Each `schema_snap!(name, T)` generates a `#[test] fn name()` that
//! snapshots `schemars::schema_for!(T)` via `insta::assert_yaml_snapshot!`.
//! Future schema drift breaks these tests; reviewer either accepts the diff
//! with `cargo insta accept --all` or fixes the regression.
//!
//! Run `cargo test -p zendriver-mcp --test schema_snapshots --all-features --locked`
//! once after editing to generate `*.snap.new` files, then accept via
//! `cargo insta accept --all`.

use insta::assert_yaml_snapshot;
use schemars::schema_for;
use zendriver_mcp::selectors;
use zendriver_mcp::tools;

macro_rules! schema_snap {
    ($name:ident, $ty:ty) => {
        #[test]
        fn $name() {
            assert_yaml_snapshot!(stringify!($name), schema_for!($ty));
        }
    };
}

// ---------- shared ---------------------------------------------------------

schema_snap!(common_empty_input, tools::common::EmptyInput);
schema_snap!(common_modifier_arg, tools::common::ModifierArg);
schema_snap!(common_blob_output, tools::common::BlobOutput);
schema_snap!(selectors_selector, selectors::Selector);

// ---------- scroll ---------------------------------------------------------

schema_snap!(scroll_page_in, tools::scroll::PageScrollInput);
schema_snap!(scroll_page_out, tools::scroll::PageScrollOutput);

// ---------- window ---------------------------------------------------------

schema_snap!(window_state_dto, tools::window::WindowStateDto);
schema_snap!(window_bounds_dto, tools::window::WindowBoundsDto);
schema_snap!(window_set_mode, tools::window::SetWindowMode);
schema_snap!(window_set_in, tools::window::SetWindowInput);

// ---------- pdf ------------------------------------------------------------

schema_snap!(pdf_in, tools::pdf::PdfInput);
schema_snap!(pdf_save_mhtml_in, tools::pdf::SaveMhtmlInput);

// ---------- mouse ----------------------------------------------------------

schema_snap!(mouse_action, tools::mouse::MouseAction);
schema_snap!(mouse_in, tools::mouse::MouseInput);

// ---------- lifecycle ------------------------------------------------------

schema_snap!(lifecycle_open_in, tools::lifecycle::OpenInput);
schema_snap!(lifecycle_open_out, tools::lifecycle::OpenOutput);
schema_snap!(lifecycle_close_out, tools::lifecycle::CloseOutput);
schema_snap!(lifecycle_status_out, tools::lifecycle::StatusOutput);
schema_snap!(lifecycle_tab_summary, tools::lifecycle::TabSummary);

// ---------- navigation -----------------------------------------------------

schema_snap!(navigation_nav_out, tools::navigation::NavOutput);
schema_snap!(navigation_wait_for, tools::navigation::WaitFor);
schema_snap!(navigation_goto_in, tools::navigation::GotoInput);
schema_snap!(navigation_history_in, tools::navigation::HistoryInput);
schema_snap!(navigation_reload_in, tools::navigation::ReloadInput);
schema_snap!(navigation_ready_state_arg, tools::navigation::ReadyStateArg);
schema_snap!(
    navigation_wait_for_load_in,
    tools::navigation::WaitForLoadInput
);
schema_snap!(navigation_idle_in, tools::navigation::IdleInput);
schema_snap!(navigation_idle_out, tools::navigation::IdleOutput);

// ---------- tabs -----------------------------------------------------------

schema_snap!(tabs_tab_summary, tools::tabs::TabSummary);
schema_snap!(tabs_list_out, tools::tabs::TabListOutput);
schema_snap!(tabs_new_in, tools::tabs::TabNewInput);
schema_snap!(tabs_switch_in, tools::tabs::TabSwitchInput);
schema_snap!(tabs_close_in, tools::tabs::TabCloseInput);
schema_snap!(tabs_close_out, tools::tabs::TabCloseOutput);
schema_snap!(tabs_activate_in, tools::tabs::TabActivateInput);
schema_snap!(tabs_activate_out, tools::tabs::TabActivateOutput);

// ---------- frames ---------------------------------------------------------

schema_snap!(frames_frame_summary, tools::frames::FrameSummary);
schema_snap!(frames_list_out, tools::frames::FrameListOutput);
schema_snap!(frames_frame_goto_in, tools::frames::FrameGotoInput);

// ---------- stealth --------------------------------------------------------

schema_snap!(stealth_set_in, tools::stealth::SetStealthProfileInput);
schema_snap!(stealth_set_out, tools::stealth::SetStealthProfileOutput);
schema_snap!(stealth_set_user_agent_in, tools::stealth::SetUserAgentInput);

// ---------- find -----------------------------------------------------------

schema_snap!(find_bounding_box, tools::find::BoundingBox);
schema_snap!(find_element_descriptor, tools::find::ElementDescriptor);
schema_snap!(find_in, tools::find::FindInput);
schema_snap!(find_out, tools::find::FindOutput);
schema_snap!(find_all_in, tools::find::FindAllInput);
schema_snap!(find_all_out, tools::find::FindAllOutput);

// ---------- reads ----------------------------------------------------------

schema_snap!(reads_fields_preset, tools::reads::ReadFieldsPreset);
schema_snap!(reads_state_in, tools::reads::ElementStateInput);
schema_snap!(reads_state_out, tools::reads::ElementState);
schema_snap!(reads_get_links_in, tools::reads::GetLinksInput);
schema_snap!(reads_get_links_out, tools::reads::GetLinksOutput);
schema_snap!(
    reads_search_resources_in,
    tools::reads::SearchResourcesInput
);
schema_snap!(
    reads_search_resources_out,
    tools::reads::SearchResourcesOutput
);
schema_snap!(reads_resource_match, tools::reads::ResourceMatch);

// ---------- actions --------------------------------------------------------

schema_snap!(actions_action_out, tools::actions::ActionOutput);
schema_snap!(actions_ack_out, tools::actions::AckOutput);
schema_snap!(actions_mouse_button_arg, tools::actions::MouseButtonArg);
schema_snap!(actions_click_in, tools::actions::ClickInput);
schema_snap!(actions_hover_in, tools::actions::HoverInput);
schema_snap!(actions_type_in, tools::actions::TypeInput);
schema_snap!(actions_press_in, tools::actions::PressInput);
schema_snap!(actions_set_value_in, tools::actions::SetValueInput);
schema_snap!(actions_set_value_mode, tools::actions::SetValueMode);
schema_snap!(actions_clear_in, tools::actions::ClearInput);
schema_snap!(actions_clear_mode, tools::actions::ClearMode);
schema_snap!(actions_focus_in, tools::actions::FocusInput);
schema_snap!(actions_scroll_in, tools::actions::ScrollInput);
schema_snap!(actions_upload_in, tools::actions::UploadInput);
schema_snap!(actions_key_step, tools::actions::KeyStep);
schema_snap!(actions_key_sequence_in, tools::actions::KeySequenceInput);

// ---------- download -------------------------------------------------------

schema_snap!(download_in, tools::download::DownloadInput);
schema_snap!(download_out, tools::download::DownloadOutput);
schema_snap!(download_set_path_in, tools::download::SetDownloadPathInput);

// ---------- snapshot -------------------------------------------------------

schema_snap!(snapshot_html_in, tools::snapshot::HtmlInput);
schema_snap!(snapshot_img_format, tools::snapshot::ImgFormat);
schema_snap!(snapshot_screenshot_in, tools::snapshot::ScreenshotInput);

// ---------- eval -----------------------------------------------------------

schema_snap!(eval_in, tools::eval::EvalInput);
schema_snap!(eval_out, tools::eval::EvalOutput);

// ---------- cookies --------------------------------------------------------

schema_snap!(cookies_same_site_dto, tools::cookies::SameSiteDto);
schema_snap!(cookies_cookie_dto, tools::cookies::CookieDto);
schema_snap!(cookies_get_in, tools::cookies::CookiesGetInput);
schema_snap!(cookies_get_out, tools::cookies::CookiesGetOutput);
schema_snap!(cookies_set_in, tools::cookies::CookiesSetInput);
schema_snap!(cookies_set_out, tools::cookies::CookiesSetOutput);
schema_snap!(cookies_delete_in, tools::cookies::CookiesDeleteInput);
schema_snap!(cookies_delete_out, tools::cookies::CookiesDeleteOutput);
schema_snap!(cookies_clear_out, tools::cookies::CookiesClearOutput);
schema_snap!(cookies_persist_direction, tools::cookies::PersistDirection);
schema_snap!(cookies_persist_in, tools::cookies::CookiesPersistInput);
schema_snap!(cookies_persist_out, tools::cookies::CookiesPersistOutput);

// ---------- storage --------------------------------------------------------

schema_snap!(storage_kind, tools::storage::StorageKind);
schema_snap!(storage_get_in, tools::storage::StorageGetInput);
schema_snap!(storage_get_out, tools::storage::StorageGetOutput);
schema_snap!(storage_set_in, tools::storage::StorageSetInput);
schema_snap!(storage_set_out, tools::storage::StorageSetOutput);
schema_snap!(storage_delete_in, tools::storage::StorageDeleteInput);
schema_snap!(storage_delete_out, tools::storage::StorageDeleteOutput);
schema_snap!(storage_clear_in, tools::storage::StorageClearInput);
schema_snap!(storage_clear_out, tools::storage::StorageClearOutput);

// ---------- request --------------------------------------------------------

schema_snap!(request_method, tools::request::HttpMethod);
schema_snap!(request_in, tools::request::RequestInput);
schema_snap!(request_out, tools::request::RequestOutput);

// ---------- fingerprints (feature-gated) ----------------------------------

#[cfg(feature = "fingerprints")]
mod fingerprints_snaps {
    use super::*;

    schema_snap!(fingerprints_source, tools::fingerprints::FpSource);
    schema_snap!(fingerprints_generate_in, tools::fingerprints::GenerateInput);
    schema_snap!(
        fingerprints_generate_out,
        tools::fingerprints::GenerateOutput
    );
}

// ---------- monitor (feature-gated) ---------------------------------------

#[cfg(feature = "monitor")]
mod monitor_snaps {
    use super::*;

    schema_snap!(monitor_event, zendriver_mcp::state::MonitorEvent);
    schema_snap!(monitor_start_in, tools::monitor::StartInput);
    schema_snap!(monitor_start_out, tools::monitor::StartOutput);
    schema_snap!(monitor_read_in, tools::monitor::ReadInput);
    schema_snap!(monitor_read_out, tools::monitor::ReadOutput);
    schema_snap!(monitor_stop_in, tools::monitor::StopInput);
    schema_snap!(monitor_stop_out, tools::monitor::StopOutput);
}

// ---------- interception (feature-gated) ----------------------------------

#[cfg(feature = "interception")]
mod intercept_snaps {
    use super::*;

    schema_snap!(intercept_action, tools::intercept::InterceptAction);
    schema_snap!(intercept_add_in, tools::intercept::AddRuleInput);
    schema_snap!(intercept_add_out, tools::intercept::AddRuleOutput);
    schema_snap!(intercept_remove_in, tools::intercept::RemoveRuleInput);
    schema_snap!(intercept_remove_out, tools::intercept::RemoveRuleOutput);
    schema_snap!(intercept_list_out, tools::intercept::ListRulesOutput);
    schema_snap!(intercept_rule_summary, tools::intercept::RuleSummary);
    schema_snap!(intercept_clear_out, tools::intercept::ClearRulesOutput);
}

// ---------- expect (feature-gated) ----------------------------------------

#[cfg(feature = "expect")]
mod expect_snaps {
    use super::*;

    schema_snap!(expect_kind, tools::expect::ExpectKind);
    schema_snap!(expect_dialog_action, tools::expect::DialogAction);
    schema_snap!(expect_matcher, tools::expect::ExpectMatcher);
    schema_snap!(expect_register_in, tools::expect::RegisterInput);
    schema_snap!(expect_register_out, tools::expect::RegisterOutput);
    schema_snap!(expect_await_in, tools::expect::AwaitInput);
    schema_snap!(expect_await_out, tools::expect::AwaitOutput);
    schema_snap!(expect_cancel_in, tools::expect::CancelInput);
    schema_snap!(expect_cancel_out, tools::expect::CancelOutput);
}

// ---------- cloudflare (feature-gated) ------------------------------------

#[cfg(feature = "cloudflare")]
mod cloudflare_snaps {
    use super::*;

    schema_snap!(cloudflare_solve_in, tools::cloudflare::SolveInput);
    schema_snap!(cloudflare_outcome, tools::cloudflare::Outcome);
    schema_snap!(cloudflare_solve_out, tools::cloudflare::SolveOutput);
}

// ---------- imperva (feature-gated) ---------------------------------------

#[cfg(feature = "imperva")]
mod imperva_snaps {
    use super::*;

    schema_snap!(imperva_solve_in, tools::imperva::SolveImpervaInput);
    schema_snap!(imperva_outcome, tools::imperva::Outcome);
    schema_snap!(imperva_solve_out, tools::imperva::SolveImpervaOutput);
}

// ---------- datadome (feature-gated) --------------------------------------

#[cfg(feature = "datadome")]
mod datadome_snaps {
    use super::*;

    schema_snap!(datadome_solve_in, tools::datadome::SolveDataDomeInput);
    schema_snap!(datadome_outcome, tools::datadome::Outcome);
    schema_snap!(datadome_solve_out, tools::datadome::SolveDataDomeOutput);
}

// ---------- fetcher (feature-gated) ---------------------------------------

#[cfg(feature = "fetcher")]
mod fetcher_snaps {
    use super::*;

    schema_snap!(fetcher_install_in, tools::fetcher::InstallInput);
    schema_snap!(fetcher_install_out, tools::fetcher::InstallOutput);
}
