
[Setup]
AppName=Advanced PDF Reader
AppVersion=1.0.0
DefaultDirName={{pf}}\AdvancedPDFReader
DefaultGroupName=Advanced PDF Reader
OutputDir=public
OutputBaseFilename=AdvancedPDFReader-Setup-v1.0.0
SetupIconFile=assets\icons\icon.ico
Compression=lzma
SolidCompression=yes

[Files]
Source: "PDFReader-v1.0.0.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Advanced PDF Reader"; Filename: "{app}\PDFReader-v1.0.0.exe"
Name: "{userdesktop}\Advanced PDF Reader"; Filename: "{app}\PDFReader-v1.0.0.exe"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop icon"; GroupDescription: "Additional icons:"
