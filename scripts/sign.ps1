param([string]$ExePath)
$certPath = Join-Path $PSScriptRoot "..\cert.pfx"
$cert = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2($certPath, "PdfReader2026!", "Exportable,PersistKeySet")
Set-AuthenticodeSignature -FilePath $ExePath -Certificate $cert -TimestampServer "http://timestamp.digicert.com" -HashAlgorithm SHA256
