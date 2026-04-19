#ifndef DMA_H
#define DMA_H

class PhysicalRegionDescriptor;
class PRDTable;

#ifdef __cplusplus
extern "C" {
#endif

void dma_init();
void dma_reset();
bool dma_is_error();
void dma_start(int type, unsigned long baseAddress);

void prd_set_base_address(PhysicalRegionDescriptor* prd, unsigned long phyBaseAddr);
void prd_set_byte_count(PhysicalRegionDescriptor* prd, unsigned short bytes);
void prd_set_end_of_table(PhysicalRegionDescriptor* prd, bool EOT);
void prd_table_set_physical_region_descriptor(PRDTable* table, int index, PhysicalRegionDescriptor* prd, bool EOT);
unsigned long prd_table_base_address(PRDTable* table);

#ifdef __cplusplus
}
#endif
/* 
 * Physical Region Descriptor(PRD) 物理内存区描述符
 * 用于描述物理内存与外设之间进行DMA方式数据传输时，
 * 源(或目标)物理内存区域起始地址、 长度信息的数据结构。
 */
class PhysicalRegionDescriptor
{
private:
	unsigned char storage[8];

public:
	PhysicalRegionDescriptor() {
		for (int i = 0; i < 8; i++) {
			this->storage[i] = 0;
		}
	}

	void SetBaseAddress(unsigned long phyBaseAddr) {
        prd_set_base_address(this, phyBaseAddr);
    }
	void SetByteCount(unsigned short bytes) {
        prd_set_byte_count(this, bytes);
    }
	void SetEndOfTable(bool EOT /* End of Table */ ) {
        prd_set_end_of_table(this, EOT);
    }
	
};


/* 
 * 物理内存区描述符(PRD)表，一个或多个PRD可以构成描述符表，
 * 每个表项描述一块用于DMA传输的物理内存区域，由此可以描述
 * 物理内存上不连续的多个区域用于同一次DMA传输。
 * 
 * 表中最后一项物理内存区描述符的Bit(31)为1表示PRD Table的结束，
 * 每启动一次DMA传输时，DMA控制芯片从PRD Table的第0项开始，依次
 * 读/写表中每一个PRD描述的内存区域，直至DMA芯片检测到表中第n个PRD
 * 的Bit(31)为1，则认为PRD Table结束，才算完成DMA传输。
 * 
 * 注: PRD Table中相邻两个描述符，它们所描述的物理内存区可以是不连续
 * 的，但是这两个描述符自身必须在内存上是连续的，因此PRDTable中
 * 使用数组的形式来实现描述符表。
 */
class PRDTable
{
private:
	unsigned char storage[80] __attribute__((aligned(4)));

	/* Members */
public:
	PRDTable() {
		for (int i = 0; i < 80; i++) {
			this->storage[i] = 0;
		}
	}

	/* 设置index相应的物理内存区描述符(PRD) */
	void SetPhysicalRegionDescriptor(int index, PhysicalRegionDescriptor& prd, bool EOT /* End of Table */) {
        prd_table_set_physical_region_descriptor(this, index, &prd, EOT);
    }
	
	/* 获取PRD Table的物理起始地址 (注：返回的是物理地址，而不是线性地址) */
	unsigned long GetPRDTableBaseAddress() {
        return prd_table_base_address(this);
    }
};

/* 
 * DMA类封装了DMA控制芯片的内部寄存器的端口号，
 * 包括命令寄存器，状态寄存器，以及物理区域
 * 描述符表寄存器(PRDTR)的端口号。
 * 
 * 同时还对这些寄存器中特定比特位定义相应的常量。
 */
class DMA
{
public:
	enum DMAType	/* 命令寄存器Bit(3), Read/Write位；告诉DMA控制芯片进行DMA传输的方向 */
	{
		READ	=	0x08,		/* DMA Read，Bit(3) = 1；表示读硬盘，写入内存 */
		WRITE	=	0x00		/* DMA Write，Bit(3) = 0；表示写硬盘，读内存 */
	};
public:
	static void Init() {
        dma_init();
    }			/* 初始化DMA芯片，确定DMA控制芯片内部寄存器所占据的I/O端口号 */

	static void Reset() {
        dma_reset();
    }		/* 重设DMA控制芯片，清除前一次DMA传输的结果状态 */
	
	static bool IsError() {
        return dma_is_error();
    }		/* 检查前一次DMA执行过程中是否出错 */

	/* 根据参数规定的DMA类型、PRD Table的起始物理地址，启动DMA操作 */
	static void Start(enum DMAType type, unsigned long baseAddress) {
        dma_start(type, baseAddress);
    }
};

#endif
