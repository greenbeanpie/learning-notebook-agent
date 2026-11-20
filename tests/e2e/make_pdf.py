"""Write a minimal one-page text PDF for the PDF ingestion check: python make_pdf.py out.pdf"""
import sys

objs = [
    "<</Type/Catalog/Pages 2 0 R>>",
    "<</Type/Pages/Kids[3 0 R]/Count 1>>",
    "<</Type/Page/Parent 2 0 R/MediaBox[0 0 300 144]/Contents 4 0 R/Resources<</Font<</F1 5 0 R>>>>>>",
    None,
    "<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>",
]
stream = b"BT /F1 18 Tf 20 100 Td (A basis spans the vector space and is independent.) Tj ET"
out = b"%PDF-1.4\n"
offsets = []
for i, o in enumerate(objs, 1):
    offsets.append(len(out))
    body = (b"<</Length %d>>stream\n" % len(stream) + stream + b"\nendstream") if o is None else o.encode()
    out += b"%d 0 obj\n" % i + body + b"\nendobj\n"
xref = len(out)
out += b"xref\n0 6\n0000000000 65535 f \n" + b"".join(b"%010d 00000 n \n" % o for o in offsets)
out += b"trailer\n<</Size 6/Root 1 0 R>>\nstartxref\n%d\n%%%%EOF\n" % xref
open(sys.argv[1] if len(sys.argv) > 1 else "t.pdf", "wb").write(out)
