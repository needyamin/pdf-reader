$thumb = "231DBF0059E9CF521447B601C577EE8F379B991F"
$cert = Get-Item "Cert:\CurrentUser\My\$thumb"

# Add to Trusted Root (will show a security dialog - click YES)
$rootStore = New-Object System.Security.Cryptography.X509Certificates.X509Store("Root", "CurrentUser")
$rootStore.Open("ReadWrite")
$rootStore.Add($cert)
$rootStore.Close()
Write-Host "Added to Trusted Root CA"

# Add to Trusted Publisher
$pubStore = New-Object System.Security.Cryptography.X509Certificates.X509Store("TrustedPublisher", "CurrentUser")
$pubStore.Open("ReadWrite")
$pubStore.Add($cert)
$pubStore.Close()
Write-Host "Added to Trusted Publisher"
