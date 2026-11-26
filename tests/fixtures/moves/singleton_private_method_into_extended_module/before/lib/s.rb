class Store
  class << self
    def size
      count + 1
    end

    private

    def count
      rows.length * 2
    end
  end
end
