
pkgname=xod
pkgver=0.1.0
pkgrel=1
pkgdesc="A small modal GUI text editor with switchable themes"
arch=('x86_64')
url="https://github.com/abderrahimebelazreg/xod"
license=('MIT')
depends=('gcc-libs' 'glibc')
makedepends=('rust' 'cargo')
source=("git+https://github.com/abderrahimebelazreg/xod.git#tag=v${pkgver}")
sha256sums=('SKIP')

prepare() {
    cd "$srcdir/$pkgname"
    cargo fetch --locked
}

build() {
    cd "$srcdir/$pkgname"
    cargo build --release --locked
}

package() {
    cd "$srcdir/$pkgname"

    install -Dm755 target/release/xod \
        "$pkgdir/usr/bin/xod"

    install -Dm644 assets/xod.desktop \
        "$pkgdir/usr/share/applications/xod.desktop"

    install -Dm644 assets/xod-256.png \
        "$pkgdir/usr/share/icons/hicolor/256x256/apps/xod.png"

    install -Dm644 LICENSE \
        "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
}
