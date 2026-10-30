# Test na dva računara

Isti program, `remote-desk.exe`, pokreće se na **oba** računara. Jedan deli ekran (HOST), drugi ga gleda i
upravlja njime (VIEWER). Instalacija nije potrebna; dovoljan je jedan fajl.

## 1. Preuzmi program

Na drugi računar prenesi samo fajl **`dist\remote-desk.exe`** (7 MB), bilo kako:

- `git clone https://github.com/100lejeukuci/100desktops.git` pa uzmi `dist\remote-desk.exe`, ili kasnije `git pull`
- na GitHub stranici repoa: **Code → Download ZIP**, pa raspakuj
- USB / deljeni folder

Na oba računara koristi **istu, novu verziju**. Windows SmartScreen može upozoriti jer program nije potpisan:
klikni **More info → Run anyway**.

## 2. Računar koji deli ekran (HOST)

Host mora imati GPU sa hardverskim H.264 enkoderom (AMD, NVIDIA ili Intel). Računar na kome je ovo razvijeno
(RX 6900 XT) je proveren.

1. Dvoklik na `remote-desk.exe`. Gore piše **tvoja IP adresa** (npr. `192.168.100.72`). To je adresa koju upisuješ na viewer-u.
2. Samo prvi put: u kartici HOST klikni **Dozvoli u firewall-u**, pa **Da** u Windows prozoru (UAC).
   Status treba da pređe u „Firewall: dozvoljeno". Bez ovoga Windows blokira dolazne veze.
3. Klikni **Pokreni hosting**. Status je „Čeka viewer-a na ...".

## 3. Računar koji gleda i upravlja (VIEWER)

1. Dvoklik na `remote-desk.exe`.
2. U polje **HOST IP** upiši adresu koju host prikazuje (npr. `192.168.100.72`) i klikni **Poveži se**.
   Status je „Host je primio zahtev. Na tom računaru treba da klikneš Dozvoli".

## 4. Opet na hostu: odobri vezu

Prozor hosta iskoči sa pitanjem. Izaberi:

- **Dozvoli gledanje i kontrolu**: viewer vidi ekran i upravlja mišem i tastaturom
- **Dozvoli samo gledanje**
- **Odbij**

Viewer otvara prozor sa ekranom hosta. Miš i tastatura u tom prozoru upravljaju hostom.

- **Hitni prekid kontrole na hostu:** `Ctrl+Alt+Shift+F12`
- Prekid sa viewer strane: dugme **Prekini**, ili zatvori prozor sa ekranom
- Prekid sa host strane: dugme **Zaustavi hosting**

Oba računara mogu biti i host i viewer, isti program.

## Šta gledati i javiti

U kartici VIEWER, kad je povezan, vidi se:

| Polje | Značenje | Dobro je |
|---|---|---|
| FPS | kadrova u sekundi | oko 60 (ekran koji se ne menja ne šalje kadrove; pusti video ili pomeraj prozor) |
| Protok | Mbit/s | oko 20 |
| Kašnjenje (proc.) | od hvatanja ekrana na hostu do prikaza; **procena** jer satovi dva računara nisu isti | što manje; na kablu nekoliko ms |
| RTT | kružno kašnjenje mreže | na kablu ispod 1 ms |

Ispod toga: koliko paketa je moralo da se pošalje ponovo i koliko kadrova je odbačeno (treba da bude blizu 0).

Kad probaš, javi: tip veze (kabl/Wi-Fi), FPS, kašnjenje, RTT, i da li miš „zaostaje". Ako nešto zapne, pošalji i
tekst iz kartice **DNEVNIK**.

## Ako ne radi

| Simptom | Šta uraditi |
|---|---|
| „Host je primio zahtev..." a na hostu nema dijaloga | Prozor hosta je minimizovan ili iza drugih; trebalo bi da iskoči sam. Proveri traku zadataka |
| „Nema odgovora od ..." posle 5 s | Zahtev ne stiže do hosta: (1) host je pokrenut i IP je tačno ona koju host prikazuje, (2) oba računara su u istoj mreži (prve tri cifre IP adrese iste), Wi-Fi „guest" mreža i „AP isolation" blokiraju, (3) firewall na **hostu** je „dozvoljeno" |
| „Računar odgovara, ali na tom portu ne radi remote-desk host" | IP je ispravna, ali host nije pokrenut ili je port drugačiji; proveri polje Port na oba računara |
| Host: „ne mogu da otvorim UDP port" | Port je zauzet; zatvori drugi remote-desk ili promeni port (isti na oba računara) |
| Host: „no hardware H.264 encoder found" | Taj računar nema hardverski enkoder; zameni uloge ili ažuriraj drajver grafičke |
| Slika se vidi, ali miš ne reaguje | Host je izabrao „samo gledanje", ili upravljaš UAC/admin prozorom (još ne radi) |
| Program se ne otvara uopšte | Pogledaj `%TEMP%\remote-desk-crash.log` i pošalji ga |

## Poznata ograničenja ove verzije

- Nema enkripcije ni provere identiteta; koristi samo na mreži kojoj veruješ.
- Samo prvi monitor; ne prikazuje se kursor sa hosta (vidiš svoj lokalni kursor).
- Win, Alt+Tab i Ctrl+Alt+Del ostaju lokalni na viewer-u; UAC prozori i login ekran se ne mogu kontrolisati.
- Radi u istoj mreži; za internet treba još posla (signaling i NAT traversal).

Komandna linija (`dist\remote-desk-cli.exe`) postoji za testove i skripte; za svakodnevno korišćenje nije potrebna.
