# remote-desk

Prototip aplikacije za daljinski pristup ekranu (zamena za AnyDesk) za **Windows 11**, napisan u Rustu.
Cilj je što manja latencija: hvatanje ekrana, kodiranje i dekodiranje rade na GPU-u, a prenos ide
preko sopstvenog UDP protokola.

## Šta radi

- Prozor sa tvojom IP adresom, poljem **HOST IP** za povezivanje na drugi računar, i dugmadima za hosting i povezivanje
- Hvatanje ekrana (DXGI Desktop Duplication), 60 fps
- Hardverski H.264 enkoder (Media Foundation: AMD, NVIDIA, Intel) i GPU dekoder
- Sopstveni UDP protokol: deljenje kadra na pakete, oporavak od gubitka preko NACK-a (ponovno slanje) i zahteva za keyframe
- Daljinska kontrola miša i tastature (`SendInput`), sa pouzdanim slanjem tastera i dugmadi
- Host odobrava svaku vezu (gledanje i kontrola, samo gledanje, ili odbijanje); `Ctrl+Alt+Shift+F12` na hostu je hitni prekid
- Dugme za dozvolu u firewall-u (traži administratorsko odobrenje) i provera da li je pravilo podešeno

Izmereno na jednom računaru (loopback, AMD RX 6900 XT, 1920x1080): 60 fps, oko 6,5 ms od hvatanja do prikaza,
60 fps i pri 1% gubitka paketa. **Nije još testirano na dva računara.**

## Pokretanje

Gotov program je **`dist/remote-desk.exe`** (bez instalacije; isti na oba računara).
Korak po korak za test na dva računara: [TEST-2PC.md](TEST-2PC.md).

`dist/remote-desk-cli.exe` je verzija za komandnu liniju (testovi i skripte):

```
remote-desk-cli host [--port 7000]          # deli ekran, pita u konzoli da li da dozvoli vezu
remote-desk-cli view <ip-hosta>:7000        # gleda i kontroliše host
remote-desk-cli local                       # benchmark na jednom računaru (bez mreže)
remote-desk-cli probe                       # test prozor koji ispisuje primljeni unos
```

GUI prima i opcione argumente: `--host` (odmah pokreni hosting), `--connect <ip[:port]>`, `--port <n>`.

## Build

Potrebni su [Rust](https://rustup.rs) (MSVC toolchain) i Visual Studio Build Tools sa C++ workload-om.

```
cargo build --release
```

Izlaz je `target/release/remote-desk.exe` (GUI) i `target/release/remote-desk-cli.exe`. C runtime se linkuje statički
(`.cargo/config.toml`), pa programi rade i bez Visual C++ redistributable paketa.

## Struktura

| Fajl | Uloga |
|---|---|
| `src/main.rs`, `src/gui.rs` | prozor aplikacije (egui) |
| `src/bin/remote-desk-cli.rs` | komandna linija |
| `src/host.rs`, `src/client.rs` | sesija na hostu i na viewer-u |
| `src/capture.rs` | hvatanje ekrana (DXGI) |
| `src/encoder.rs`, `src/decoder.rs` | BGRA→NV12 na GPU-u, hardverski H.264 enkoder i dekoder |
| `src/viewer.rs` | prozor sa ekranom hosta i hvatanje miša/tastature |
| `src/proto.rs` | UDP protokol, paketizacija, reassembler, NACK |
| `src/input.rs` | pouzdano slanje unosa (viewer) i `SendInput` (host) |
| `src/status.rs`, `src/log.rs`, `src/netinfo.rs` | stanje za GUI, dnevnik, IP adrese |

## Ograničenja

- **Nema enkripcije ni provere identiteta u samom programu.** Koristi samo na mreži kojoj veruješ ili kroz Tailscale; ne otvaraj port na ruteru.
- Samo prvi monitor; nema prikaza kursora sa hosta; nema zvuka, clipboard-a ni prenosa fajlova.
- Win, Alt+Tab i Ctrl+Alt+Del ostaju lokalni na viewer-u. UAC prozori i login ekran se ne mogu kontrolisati
  (za to treba Windows servis).
- Preko interneta radi kroz Tailscale (besplatan VPN, na oba računara, isti nalog); aplikacija prepoznaje Tailscale adresu `100.x.x.x` i prikazuje je u kartici INTERNET. Ugrađeni NAT traversal (STUN/TURN) i šifrovanje tek treba napraviti.
