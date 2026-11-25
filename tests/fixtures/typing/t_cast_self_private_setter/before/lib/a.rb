# typed: true
class A
  extend T::Sig

  def reset
    self.count = 0
    self.total += 1
  end

  private

  attr_accessor :count, :total
end
