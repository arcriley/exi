#!/bin/sh
# Downloads EXIficient 1.0.7 and its run-time dependencies from Maven Central into
# fixtures/jars. Needs curl. Run from the repository root.
set -e
R=https://repo1.maven.org/maven2
D=fixtures/jars
mkdir -p "$D"
get() { curl -sSfL -o "$D/$(basename "$1")" "$R/$1"; }
get com/siemens/ct/exi/exificient/1.0.7/exificient-1.0.7.jar
get com/siemens/ct/exi/exificient-core/1.0.7/exificient-core-1.0.7.jar
get com/siemens/ct/exi/exificient-grammars/1.0.7/exificient-grammars-1.0.7.jar
get org/slf4j/slf4j-api/2.0.6/slf4j-api-2.0.6.jar
get xmlpull/xmlpull/1.1.3.1/xmlpull-1.1.3.1.jar
get xerces/xercesImpl/2.12.2/xercesImpl-2.12.2.jar
get xml-apis/xml-apis/1.4.01/xml-apis-1.4.01.jar
ls "$D"
