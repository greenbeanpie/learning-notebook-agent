ObjC.import('Foundation');
ObjC.import('Vision');
function run(argv) {
  var listPath = argv[0], outPath = argv[1];
  var text = $.NSString.stringWithContentsOfFileEncodingError($(listPath), $.NSUTF8StringEncoding, null).js;
  var paths = text.split('\n').filter(function (x) { return x.length > 0; });
  var out = [];
  for (var i = 0; i < paths.length; i++) {
    var res = null;
    try {
      var req = $.VNRecognizeTextRequest.alloc.init;
      req.recognitionLevel = 0;
      req.usesLanguageCorrection = true;
      req.recognitionLanguages = $(['zh-Hans', 'zh-Hant', 'en-US']);
      var handler = $.VNImageRequestHandler.alloc.initWithURLOptions($.NSURL.fileURLWithPath($(paths[i])), $({}));
      var ok = handler.performRequestsError($([req]), null);
      if (ok) {
        var obs = req.results;
        var lines = [];
        for (var j = 0; j < obs.count; j++) {
          var cands = obs.objectAtIndex(j).topCandidates(1);
          if (cands.count > 0) lines.push(ObjC.unwrap(cands.objectAtIndex(0).string));
        }
        res = lines.join('\n');
      }
    } catch (e) { res = null; }
    out.push(res);
  }
  $(JSON.stringify(out)).writeToFileAtomicallyEncodingError($(outPath), true, $.NSUTF8StringEncoding, null);
}
