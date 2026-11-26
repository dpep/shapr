class Store
  module Counting
    def count
      rows.length * 2
    end
  end

  extend Counting

  class << self
    def size
      count + 1
    end
  end
end
