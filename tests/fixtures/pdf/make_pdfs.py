"""Regenerates the PDF fixtures used by crates/lna-ingest/tests/pdf_quality.rs.

    python tests/fixtures/pdf/make_pdfs.py        (needs PyMuPDF: pip install pymupdf)
"""
import os
import pymupdf

HERE = os.path.dirname(os.path.abspath(__file__))


def out(name):
    return os.path.join(HERE, name)


def text_page(doc, header, footer, body_lines, size=11):
    page = doc.new_page(width=420, height=595)
    page.insert_text((40, 40), header, fontsize=9)
    y = 90
    for line in body_lines:
        page.insert_text((40, y), line, fontsize=size)
        y += 18
    page.insert_text((40, 570), footer, fontsize=9)
    return page


# 1) multi-page document: running header, numbered footer, hyphenation, bookmarks, metadata title
doc = pymupdf.open()
bodies = [
    ["Chapter one introduces the vector space and its basis.", "The dimen-", "sion theorem says every basis has the same size."],
    ["A subspace is closed under addition and scalar multiplication.", "Spanning sets generate the whole space."],
    ["Section two studies linear maps between spaces.", "The kernel measures what a map forgets."],
    ["The rank of a matrix equals the dimension of its column space.", "Rank plus nullity equals the number of columns."],
    ["Eigenvalues describe directions kept by a map.", "Diagonalization needs enough independent eigenvectors."],
    ["Orthogonal projections give least squares solutions.", "The normal equations are derived from this picture."],
]
for i, body in enumerate(bodies):
    text_page(doc, "Linear Algebra Notes - Draft", f"Page {i + 1} of {len(bodies)}", body)
doc.set_toc([[1, "Chapter 1 Vector Spaces", 1], [2, "1.1 Subspaces", 2], [1, "Chapter 2 Linear Maps", 3], [2, "2.1 Rank", 4], [2, "2.2 Eigenvalues", 5], [1, "Chapter 3 Projections", 6]])
doc.set_metadata({"title": "Linear Algebra Notes", "author": "test"})
doc.save(out("multi_page.pdf"))

# 2) encrypted (password required to open)
doc = pymupdf.open()
text_page(doc, "secret", "1", ["This page is protected by a password."])
doc.save(out("encrypted.pdf"), encryption=pymupdf.PDF_ENCRYPT_AES_256, user_pw="secret", owner_pw="owner")

# 3) scanned: pages are only images, no text layer
doc = pymupdf.open()
pix = pymupdf.Pixmap(pymupdf.csRGB, pymupdf.IRect(0, 0, 200, 100), False)
pix.clear_with(200)
for _ in range(3):
    page = doc.new_page(width=420, height=595)
    page.insert_image(pymupdf.Rect(40, 40, 380, 200), pixmap=pix)
doc.save(out("scanned.pdf"))

# 4) corrupt: a valid file cut in half
data = open(out("multi_page.pdf"), "rb").read()
open(out("corrupt.pdf"), "wb").write(data[: len(data) // 2])

# 5) not a PDF at all
open(out("garbage.pdf"), "wb").write(b"this is not a pdf" * 20)
print("fixtures written to", HERE)
