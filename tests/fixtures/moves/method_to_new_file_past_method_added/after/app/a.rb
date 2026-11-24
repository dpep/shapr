require_relative 'b'

class A
  def self.method_added(n)
    puts n
  end
end
