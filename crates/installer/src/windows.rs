//! Files that let Windows Setup find its install media when an ISO is started from the Toastrack menu.
//!
//! Windows Setup reads `autounattend.xml` from the root of removable drives and runs the commands in its
//! windowsPE-pass `RunSynchronous` list. Ours runs `toastrack\attach.exe` (from this crate's sibling
//! `toastrack-winattach`), which attaches the ISO chosen in the menu as a virtual DVD (see `toastrack_core::winreq`).

/// Where the helper lives on the data partition.
pub const HELPER_DIR: &str = "toastrack";
pub const HELPER_FILE: &str = "attach.exe";
pub const ANSWER_FILE: &str = "autounattend.xml";
/// Optional bundle file holding the helper.
pub const BUNDLE_HELPER: &str = "toastrack-attach.exe";

/// The answer file. It only adds one command to Setup's early phase; everything else in Setup stays interactive.
/// The command looks for the helper on every drive letter, because the stick's letter is not known in advance.
pub const AUTOUNATTEND: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<unattend xmlns="urn:schemas-microsoft-com:unattend">
  <settings pass="windowsPE">
    <component name="Microsoft-Windows-Setup" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
      <RunSynchronous>
        <RunSynchronousCommand wcm:action="add">
          <Order>1</Order>
          <Path>cmd /c "for %i in (C D E F G H I J K L M N O P Q R S T U V W Y Z) do @if exist %i:\toastrack\attach.exe %i:\toastrack\attach.exe"</Path>
          <Description>Toastrack: attach the ISO chosen in the boot menu so Setup finds install.wim</Description>
        </RunSynchronousCommand>
      </RunSynchronous>
    </component>
  </settings>
</unattend>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answer_file_is_well_formed_enough_and_names_the_helper() {
        assert!(AUTOUNATTEND.starts_with("<?xml"));
        assert!(AUTOUNATTEND.contains("pass=\"windowsPE\""));
        assert!(AUTOUNATTEND.contains("RunSynchronousCommand wcm:action=\"add\""));
        assert!(AUTOUNATTEND.contains(&format!("{HELPER_DIR}\\{HELPER_FILE}")));
        // every opened tag is closed (cheap structural check)
        for t in ["unattend", "settings", "component", "RunSynchronous", "RunSynchronousCommand", "Order", "Path", "Description"] {
            let opened = AUTOUNATTEND.matches(&format!("<{t}>")).count() + AUTOUNATTEND.matches(&format!("<{t} ")).count();
            assert_eq!(opened, AUTOUNATTEND.matches(&format!("</{t}>")).count(), "{t}");
        }
    }
}
