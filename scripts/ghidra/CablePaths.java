// Analyze only cable/power acquisition functions, not activation or payment.
//@category CableScope
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.symbol.Reference;
import ghidra.program.model.symbol.Symbol;
import ghidra.program.model.symbol.SymbolIterator;
import ghidra.util.task.TaskMonitor;
import java.nio.file.*;
import java.nio.charset.StandardCharsets;
import java.io.*;
import java.util.*;

public class CablePaths extends GhidraScript {
  public void run() throws Exception {
    String[] args = getScriptArgs();
    Path output = Paths.get(args[1]);
    Files.createDirectories(output);
    DecompInterface decomp = new DecompInterface();
    decomp.openProgram(currentProgram);
    try (PrintWriter report = new PrintWriter(Files.newBufferedWriter(output.resolve("paths.tsv"), StandardCharsets.UTF_8))) {
      report.println("kind\taddress\tname\tdetail");
      int index = 0;
      for (String line : Files.readAllLines(Paths.get(args[0]), StandardCharsets.UTF_8)) {
        if (line.isBlank()) continue;
        String[] fields = line.split("\t",2);
        Address address = toAddr(fields[0]);
        Function function = getFunctionAt(address);
        if (function == null) { disassemble(address); function = createFunction(address, null); }
        if (function == null) { report.println("missing\t"+address+"\t"+fields[1]+"\t"); continue; }
        report.println("target\t"+address+"\t"+fields[1]+"\t"+function.getBody().getNumAddresses());
        for (Function called : function.getCalledFunctions(monitor)) {
          report.println("callee\t"+called.getEntryPoint()+"\t"+called.getName()+"\tfrom="+address);
        }
        DecompileResults result = decomp.decompileFunction(function, 30, monitor);
        if (result.decompileCompleted()) {
          Files.writeString(output.resolve(String.format("%02d_%s.c",index++,address)),
            "/* "+fields[1]+" */\n"+result.getDecompiledFunction().getC(),StandardCharsets.UTF_8);
        } else report.println("decompile_error\t"+address+"\t"+fields[1]+"\t"+result.getErrorMessage().replace('\n',' '));
      }
      Set<String> imports = Set.of("_IOServiceMatching","_IOServiceOpen","_IOConnectCallStructMethod","_IORegistryEntryCreateCFProperties","_IORegistryEntryCreateCFProperty","_IOServiceGetMatchingServices");
      SymbolIterator symbols = currentProgram.getSymbolTable().getAllSymbols(true);
      while (symbols.hasNext()) {
        Symbol symbol = symbols.next();
        if (!imports.contains(symbol.getName())) continue;
        int count = 0;
        for (Reference ref : getReferencesTo(symbol.getAddress())) {
          if (++count > 150) break;
          Function caller = getFunctionContaining(ref.getFromAddress());
          report.println("import_ref\t"+ref.getFromAddress()+"\t"+symbol.getName()+"\t"+(caller == null ? "data" : caller.getName()));
        }
      }
    } finally { decomp.dispose(); }
    println("Cable acquisition evidence exported to " + output);
  }
}
