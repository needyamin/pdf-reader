$p = ConvertTo-SecureString "PdfReader2026!" -Force -AsPlainText
Export-PfxCertificate -Cert "Cert:\CurrentUser\My\231DBF0059E9CF521447B601C577EE8F379B991F" -FilePath "$PSScriptRoot\..\cert.pfx" -Password $p
Write-Host "cert.pfx exported"
