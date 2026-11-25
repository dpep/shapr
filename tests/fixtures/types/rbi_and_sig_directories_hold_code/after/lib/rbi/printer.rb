module Rbi
  class Printer
    def visit(node)
      return if node.nil?
      node.accept(self)
    end
  end
end
