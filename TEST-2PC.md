# Test na dva računara

Potreban je samo `dist\remote-desk.exe` (0,6 MB, bez instalacije). Ne pokreći host i viewer na istom računaru
dok kontrolišeš miš: viewer bi hostu slao komande koje pomeraju njegov sopstveni miš.

## Uloge

| Računar | Šta mu treba |
|---|---|
| **Host** (deli ekran) | Windows 10/11 sa GPU-om koji ima hardverski H.264 enkoder (AMD, NVIDIA ili Intel). Računar na kome je ovo napravljeno (RX 6900 XT) je provereno dobar. |
| **Viewer** (gleda i kontroliše) | Bilo koji Windows 10/11 računar. Windows "N" izdanja traže "Media Feature Pack". |

Oba računara moraju biti na istoj mreži (isti ruter). Kablom je bolje nego Wi-Fi-jem; na Wi-Fi-ju proveri da ruter
nema uključenu "AP/client isolation".

## 1. Host: dozvoli dolazni UDP (jednom)

Windows firewall blokira dolazni UDP dok ne dozvoliš. Otvori **PowerShell kao administrator** na hostu i pokreni:

```powershell
New-NetFirewallRule -DisplayName "Remote Desk test" -Direction Inbound -Protocol UDP -LocalPort 7000 -Action Allow
```

Posle testa ukloni pravilo:

```powershell
Remove-NetFirewallRule -DisplayName "Remote Desk test"
```

## 2. Host: pokreni

Kopiraj `remote-desk.exe` bilo gde na host i u PowerShell-u ili CMD-u pokreni:

```
remote-desk.exe host
```

Njegova IP adresa na ovoj mreži se vidi komandom `ipconfig` (red "IPv4 Address"). Na računaru na kome je ovo napravljeno je
`192.168.100.72`.

## 3. Viewer: poveži se

Kopiraj `remote-desk.exe` na viewer (USB, deljeni folder...). Windows SmartScreen može upozoriti jer fajl nije
potpisan: "More info" pa "Run anyway" (samo ako si ga sam preneo).

```
remote-desk.exe view 192.168.100.72:7000
```

## 4. Host: odobri

Na hostu će se pojaviti pitanje. Upiši:

- `c` + Enter: dozvoli gledanje i kontrolu miša/tastature
- `v` + Enter: samo gledanje
- bilo šta drugo: odbij

Viewer otvara prozor sa ekranom hosta. Miš i tastatura u tom prozoru upravljaju hostom.

**Hitni prekid na hostu:** `Ctrl+Alt+Shift+F12` odmah završava sesiju. Sesiju završava i zatvaranje viewer prozora.

## Šta gledati

Viewer ispisuje jedan red u sekundi:

```
60 fps shown | 20.3 Mbit/s | capture->present (est.) p50 .. ms p99 .. ms | RTT .. ms | chunks NACKed N | frames given up N | keyframe req N | input unacked N
```

- **fps shown**: treba oko 60. Ekran koji se ne menja ne šalje kadrove, pa za test pusti video ili pomeraj prozor
  (npr. https://www.testufo.com).
- **capture->present (est.)**: procena kašnjenja od hvatanja do prikaza. Satovi dva računara nisu isti, pa je ovo procena
  preko RTT-a i može biti netačna nekoliko ms (naročito na Wi-Fi-ju). RTT je pouzdan. Prva sekunda je uvek lošija.
- **chunks NACKed** raste kad se gube paketi; **frames given up** i **keyframe req** treba da ostanu blizu 0.
- **input unacked**: treba da bude 0 ili 1; ako raste, komande do hosta ne stižu.

Kad probaš, javi mi: tip veze (kabl/Wi-Fi), fps, p50/p99 i RTT, i da li je kontrola "osećajna" (zaostaje li miš).

## Ako ne radi

| Simptom | Verovatan uzrok |
|---|---|
| Viewer ostaje na "waiting for the host to approve" | Firewall na hostu (korak 1), pogrešna IP, ili host nije pokrenut. Proveri iz viewera: `Test-NetConnection 192.168.100.72` |
| Host kaže "no hardware H.264 encoder found" | GPU/drajver nema hardverski enkoder; zameni uloge ili ažuriraj drajver |
| Viewer se otvori pa odmah zatvori | Pogledaj grešku u konzoli i pošalji mi je |
| Sliku vidi, ali miš ne reaguje | Host je odgovorio `v`, ili je prozor kojim upravljaš u UAC/admin aplikaciji (to još ne radi) |
| Kašnjenje veliko, mnogo NACK-ova | Wi-Fi sa lošim signalom; probaj kabl |

## Poznata ograničenja ove verzije

Nema enkripcije ni provere identiteta (koristi samo na mreži kojoj veruješ), samo prvi monitor, tasteri Win/Alt+Tab/Ctrl+Alt+Del
ostaju lokalni na viewer-u, a UAC prozori i login ekran se ne mogu kontrolisati.
