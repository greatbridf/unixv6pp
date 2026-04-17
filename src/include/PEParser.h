#ifndef PE_PARSER_H
#define PE_PARSER_H

#include "INode.h"

struct pe_parser {
	unsigned long EntryPointAddress;
	unsigned long TextAddress;
	unsigned long TextSize;

	unsigned long DataAddress;
	unsigned long DataSize;
	unsigned long StackSize;
};

extern "C" struct pe_parser* PEParser_new();
extern "C" void PEParser_free(struct pe_parser*);
extern "C" bool PEParser_load_header(struct pe_parser*, Inode*);
extern "C" void PEParser_relocate(struct pe_parser*, Inode*, bool);

#endif
