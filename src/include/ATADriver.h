#ifndef ATA_DRIVE_H
#define ATA_DRIVE_H

#include "Regs.h"

extern "C" void ata_handler();
extern "C" void ata_dev_start(struct Buf* bp);

class ATADriver
{
public:
	/* 磁盘中断设备处理子程序 */
	static void ATAHandler(struct pt_regs* reg, struct pt_context* context) {
        (void)reg;
	    (void)context;
	    ata_handler();
    }

	/* 设置磁盘寄存器，启动磁盘进行I/O操作 */
	static void DevStart(struct Buf* bp) {
        ata_dev_start(bp);
    }
};

#endif
