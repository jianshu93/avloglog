#!/usr/bin/env python3
"""Regenerate packed-state/HLDLC fixtures using the pinned BBTools Java source.

Example from the crate root (a JDK and Python 3 are required):
  python3 tests/fixtures/generate_java.py work/reference/avll.java

The generated Java class takes prehashed values. It removes only parent-class,
CardStats and external-correction integration, retaining the actual state,
promotion, merge and complete HLDLC estimator functions.
"""
import argparse
from pathlib import Path
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("reference_java", type=Path)
parser.add_argument("--output", type=Path, default=Path(__file__).with_name("java_reference.csv"))
args = parser.parse_args()
src = args.reference_java.read_text()
start=src.index('\t/** Base for arithmetic encoding. */')
end=src.index('\t/*----------------     Estimation Pipeline')
# Truncate before the banner whose contents are irrelevant to the standalone estimator.
end=src.rfind('\t/*--------------------------------------------------------------*/',start,end)
core=src[start:end]
start2=src.index('\tdouble[] estimate(){')
rest=src[start2:]
core=core.replace('@Override','')
a=core.index('\tpublic void add(CardinalityTracker log){')
b=core.index('\tpublic void add(ArithmeticVariableLogLog log){',a)
core=core[:a]+core[b:]
core=core.replace('hashAndStore(long number)','addHash(long number)')
core=core.replace('final long key=hash64shift(number^hashXor);','final long key=number; // Oracle accepts an already-computed unsigned 64-bit hash.')
core=core.replace('ArithmeticVariableLogLog','AvllOracle')
core=core.replace('\t\tadded+=log.added;','')
core=core.replace('\t\tlastCardinality=-1;','\t\t// Removed parent cardinality cache:')
prefix='''// Extracted mechanically from BBTools ArithmeticVariableLogLog.java.
// Upstream license: see source repository LICENSE and the retained NOTICE in this crate.
// Changes: standalone constructor; prehashed input; omit CardStats/parent integration.
public final class AvllOracle {
  public AvllOracle(int requested) {
    modBuckets=Math.max(1,(requested+10)/11)*11;
    packedLen=modBuckets/11;
    packed=new long[packedLen]; floorCount=modBuckets;
  }
  static long splitmix(long n) {
    long z=n+0x9e3779b97f4a7c15L;
    z=(z^(z>>>30))*0xbf58476d1ce4e5b9L;
    z=(z^(z>>>27))*0x94d049bb133111ebL;
    return z^(z>>>31);
  }
  static void print(String mode,int requested,long n,int shift,AvllOracle a) {
    StringBuilder b=new StringBuilder();
    for(long w:a.packed) {if(b.length()>0)b.append(':');b.append(Long.toUnsignedString(w));}
    System.out.printf(java.util.Locale.ROOT,"%s,%d,%d,%d,%d,%.17g,%s%n",mode,requested,n,shift,a.globalNLZ,a.estimate()[1],b.toString());
  }
  public static void main(String[] args) {
    System.out.println("# BBTools Java oracle, commit efe4f7b0317d34488a344045383a69c9af908b8d, HLDLC double output");
    System.out.println("# mode,requested_registers,count,hash_right_shift,global_nlz,estimate,packed_words");
    for(int requested:new int[]{11,22,88,352}) {
      AvllOracle a=new AvllOracle(requested); int previous=0;
      for(int n:new int[]{0,1,2,8,32,128,512,2048,8192,32768,131072}) {
        for(int i=previous;i<n;i++)a.addHash(splitmix(i));
        print("stream",requested,n,0,a); previous=n;
      }
    }
    for(int shift:new int[]{8,16,24,32,40,48}) {
      AvllOracle a=new AvllOracle(88);
      for(int i=0;i<65536;i++)a.addHash(splitmix(i)>>>shift);
      print("stream",88,65536,shift,a);
    }
    for(int requested:new int[]{22,88}) {
      AvllOracle a=new AvllOracle(requested),b=new AvllOracle(requested);
      for(int i=0;i<32768;i++) {
        if((i&1)==0)a.addHash(splitmix(i)); else b.addHash(splitmix(i));
      }
      a.add(b);print("merge",requested,32768,0,a);
    }
  }
'''
with tempfile.TemporaryDirectory(prefix="avll-java-oracle-") as directory:
    java = Path(directory) / "AvllOracle.java"
    java.write_text(prefix + core + rest)
    subprocess.run(["javac", str(java)], check=True)
    result = subprocess.run(["java", "-cp", directory, "AvllOracle"], check=True, capture_output=True, text=True)
    args.output.write_text(result.stdout)
