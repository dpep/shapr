module Rbi
  class Printer
    def visit(node)
      node.accept(self)
    end
  end
end
