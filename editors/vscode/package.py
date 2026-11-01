"""Package the dependency-free extension as a local VSIX using only the stdlib."""
from pathlib import Path
from zipfile import ZipFile, ZIP_DEFLATED
import json

root = Path(__file__).resolve().parent
package = json.loads((root / "package.json").read_text())
version = package["version"]
manifest = f'''<?xml version="1.0" encoding="utf-8"?>
<PackageManifest Version="2.0.0" xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011">
 <Metadata><Identity Language="en-US" Id="qlive-preview" Version="{version}" Publisher="qlive-local"/>
 <DisplayName>qlive Preview</DisplayName><Description xml:space="preserve">Quarto preview from unsaved buffers</Description>
 <Properties><Property Id="Microsoft.VisualStudio.Code.Engine" Value="^1.85.0"/>
 <Property Id="Microsoft.VisualStudio.Code.ExtensionDependencies" Value=""/>
 <Property Id="Microsoft.VisualStudio.Code.ExtensionPack" Value=""/>
 <Property Id="Microsoft.VisualStudio.Code.LocalizedLanguages" Value=""/>
 <Property Id="Microsoft.VisualStudio.Code.EnabledApiProposals" Value=""/>
 <Property Id="Microsoft.VisualStudio.Code.ExecutesCode" Value="true"/></Properties></Metadata>
 <Installation><InstallationTarget Id="Microsoft.VisualStudio.Code"/></Installation>
 <Dependencies/><Assets><Asset Type="Microsoft.VisualStudio.Code.Manifest" Path="extension/package.json" Addressable="true"/></Assets>
</PackageManifest>'''
types = '''<?xml version="1.0" encoding="utf-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="json" ContentType="application/json"/><Default Extension="js" ContentType="application/javascript"/>
<Default Extension="md" ContentType="text/markdown"/><Default Extension="vsixmanifest" ContentType="text/xml"/>
</Types>'''
target = root / f"qlive-preview-{version}.vsix"
with ZipFile(target, "w", ZIP_DEFLATED) as archive:
    archive.writestr("[Content_Types].xml", types)
    archive.writestr("extension.vsixmanifest", manifest)
    for name in ["package.json", "extension.js", "README.md", "ROADMAP.md"]:
        archive.write(root / name, f"extension/{name}")
print(target)
